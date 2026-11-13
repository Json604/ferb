//! Python's `pathlib` and `os.path` as the watchers and the exclusions use them (Python 3.12, POSIX): `Path(text)`'s
//! normal form, `expanduser`, `os.path.realpath` (non-strict and strict) and `Path.resolve`.
//!
//! One deliberate difference (ENG-6): Python's `Path.resolve()` raises `RuntimeError` on a symbolic-link loop, which
//! neither the exclusions (`exclusions.py:47-50`, which catch `OSError` only) nor `Attribution.ferb_touched` nor
//! `valid_folders` catch: a file event under a loop, or a loop in `[privacy] exclude_paths`, raised out of the trail.
//! Here [`resolve`] reports the loop as `Err(Loop)` and each caller treats it as the `OSError` it already handles.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// `Path.resolve()` met a symbolic-link loop (Python: `RuntimeError("Symlink loop from ...")`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loop(pub String);

/// `str(Path(text))`: separators collapsed (two leading ones kept), `.` parts and a trailing `/` dropped; "" is ".".
pub fn normal(text: &str) -> String {
    if text.is_empty() {
        return ".".to_string();
    }
    let lead = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = text
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let joined = parts.join("/");
    if lead.is_empty() && joined.is_empty() {
        ".".to_string()
    } else {
        format!("{lead}{joined}")
    }
}

/// The parts of a normal path (`Path.parts`), the root as "/".
pub fn parts(text: &str) -> Vec<String> {
    let n = normal(text);
    let mut out = Vec::new();
    if n.starts_with("//") {
        out.push("//".to_string());
    } else if n.starts_with('/') {
        out.push("/".to_string());
    }
    out.extend(
        n.split('/')
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string()),
    );
    if out.is_empty() {
        out.push(".".to_string());
    }
    out
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// `Path(text).expanduser()` as a normal path: `~` and `~/…` by `$HOME`. `~user` is left as it is (Python looks the
/// account up; FERB's settings name the owner's own home).
pub fn expanduser(text: &str) -> String {
    let n = normal(text);
    if n == "~" {
        return normal(&home());
    }
    if let Some(rest) = n.strip_prefix("~/") {
        return normal(&format!("{}/{rest}", home().trim_end_matches('/')));
    }
    n
}

fn lstat_is_link(path: &str) -> Option<bool> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| m.file_type().is_symlink())
}

fn split(path: &str) -> (String, String) {
    // os.path.split: head without trailing slashes (unless all slashes), tail after the last "/"
    match path.rfind('/') {
        Some(i) => {
            let (head, tail) = (&path[..=i], &path[i + 1..]);
            let trimmed = head.trim_end_matches('/');
            let head = if trimmed.is_empty() { head } else { trimmed };
            (head.to_string(), tail.to_string())
        }
        None => (String::new(), path.to_string()),
    }
}

fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// posixpath._joinrealpath (3.12).
fn join_real(
    mut path: String,
    rest: &str,
    strict: bool,
    seen: &mut std::collections::HashMap<String, Option<String>>,
    depth: usize,
) -> Result<(String, bool), std::io::Error> {
    let mut rest = rest.to_string();
    if rest.starts_with('/') {
        rest = rest[1..].to_string();
        path = "/".to_string();
    }
    if depth > 256 {
        return Ok((join(&path, &rest), false));
    }
    while !rest.is_empty() {
        let (name, tail) = match rest.split_once('/') {
            Some((n, t)) => (n.to_string(), t.to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = tail;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if !path.is_empty() {
                let (head, last) = split(&path);
                path = head;
                if last == ".." {
                    path = join(&join(&path, ".."), "..");
                }
            } else {
                path = "..".to_string();
            }
            continue;
        }
        let newpath = join(&path, &name);
        let is_link = match lstat_is_link(&newpath) {
            Some(l) => l,
            None => {
                if strict {
                    std::fs::symlink_metadata(&newpath)?;
                }
                false
            }
        };
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(found) = seen.get(&newpath) {
            if let Some(p) = found {
                path = p.clone();
                continue;
            }
            if strict {
                std::fs::metadata(&newpath)?;
            }
            return Ok((join(&newpath, &rest), false));
        }
        seen.insert(newpath.clone(), None);
        let target = std::fs::read_link(&newpath)?;
        let target = String::from_utf8_lossy(target.as_os_str().as_bytes()).into_owned();
        let (p, ok) = join_real(path, &target, strict, seen, depth + 1)?;
        path = p;
        if !ok {
            return Ok((join(&path, &rest), false));
        }
        seen.insert(newpath, Some(path.clone()));
    }
    Ok((path, true))
}

fn normpath(path: &str) -> String {
    // posixpath.normpath
    if path.is_empty() {
        return ".".to_string();
    }
    let initial = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    let mut out: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && out.is_empty()) || out.last() == Some(&"..") {
            out.push(comp);
        } else if !out.is_empty() {
            out.pop();
        }
    }
    let joined = out.join("/");
    let s = format!("{}{joined}", "/".repeat(initial));
    if s.is_empty() {
        ".".to_string()
    } else {
        s
    }
}

fn abspath(path: &str) -> String {
    if path.starts_with('/') {
        normpath(path)
    } else {
        let cwd = std::env::current_dir()
            .map(|p| String::from_utf8_lossy(p.as_os_str().as_bytes()).into_owned())
            .unwrap_or_else(|_| "/".to_string());
        normpath(&join(&cwd, path))
    }
}

/// `os.path.realpath(path, strict=strict)`.
pub fn realpath(path: &str, strict: bool) -> Result<String, std::io::Error> {
    let mut seen = std::collections::HashMap::new();
    let (p, _) = join_real(String::new(), path, strict, &mut seen, 0)?;
    Ok(abspath(&p))
}

fn is_loop(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ELOOP)
}

/// `Path(text).resolve()` (non-strict): `Err(Loop)` where Python raises `RuntimeError` for a link loop.
pub fn resolve(text: &str) -> Result<String, Loop> {
    let n = normal(text);
    let s = match realpath(&n, false) {
        Ok(s) => s,
        Err(e) if is_loop(&e) => return Err(Loop(n)),
        Err(_) => n.clone(),
    };
    if let Err(e) = std::fs::metadata(&s) {
        if is_loop(&e) {
            return Err(Loop(s));
        }
    }
    Ok(s)
}

/// `Path(text).resolve(strict=True)`: `Err` for a missing part (and, here, for a loop: see the module's note).
pub fn resolve_strict(text: &str) -> Result<String, std::io::Error> {
    realpath(&normal(text), true)
}

/// `os.path.lexists`.
pub fn lexists(text: &str) -> bool {
    std::fs::symlink_metadata(text).is_ok()
}

/// `x == p or x in p.parents` for normal absolute paths, by parts.
pub fn within(p: &str, x: &str) -> bool {
    let (pp, xp) = (parts(p), parts(x));
    pp.len() >= xp.len() && pp[..xp.len()] == xp[..]
}

/// A `PathBuf` from Python text.
pub fn buf(text: &str) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(text.as_bytes()))
}

/// Python text of a path.
pub fn text(p: &Path) -> String {
    String::from_utf8_lossy(p.as_os_str().as_bytes()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_normalize_as_pathlib_does() {
        assert_eq!(normal("a//b/./c/"), "a/b/c");
        assert_eq!(normal("//x/y"), "//x/y");
        assert_eq!(normal("///x"), "/x");
        assert_eq!(normal(""), ".");
        assert_eq!(normal("./"), ".");
        assert_eq!(normal("/a/../b"), "/a/../b");
        assert_eq!(parts("/a/b"), vec!["/", "a", "b"]);
        assert!(within("/a/b/c", "/a/b"));
        assert!(within("/a/b", "/a/b"));
        assert!(!within("/a/bc", "/a/b"));
        assert_eq!(normpath("/a/../../b"), "/b");
        assert_eq!(normpath("a/../.."), "..");
    }
}
