//! The path text Python's `pathlib` and `os.path` produce (CPython 3.12, POSIX), so every path FERB shows, records or
//! compares is spelled as the Python daemon spelled it. Paths are `str` here: APFS keeps names as UTF-8, so a Mac path
//! is always valid UTF-8 text (a non-UTF-8 name on another file system is read lossily: see the crate's differences).

use std::ffi::CString;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

/// `str(Path(text))`: separators collapsed, `.` parts dropped, a trailing `/` dropped, `//` kept as POSIX's own root,
/// `.` for nothing (pathlib `_parse_path` + `_format_parsed_parts`).
pub fn norm(text: &str) -> String {
    let (root, rel) = split_root(text);
    let parts: Vec<&str> = rel
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let out = format!("{root}{}", parts.join("/"));
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}

/// posixpath.splitroot without the drive: ("", "/" or "//", rest).
fn split_root(p: &str) -> (&str, &str) {
    if !p.starts_with('/') {
        ("", p)
    } else if !p[1..].starts_with('/') || p[2..].starts_with('/') {
        ("/", &p[1..])
    } else {
        ("//", &p[2..])
    }
}

/// The parts after the root, as pathlib keeps them.
pub fn tail(p: &str) -> Vec<&str> {
    let (_, rel) = split_root(p);
    rel.split('/')
        .filter(|x| !x.is_empty() && *x != ".")
        .collect()
}

fn root_of(p: &str) -> &str {
    split_root(p).0
}

pub fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// `Path(a) / b`: `b` when it is absolute, else `a`'s root and parts followed by `b`'s parts.
pub fn join(a: &str, b: &str) -> String {
    if is_abs(b) {
        return norm(b);
    }
    let a = norm(a);
    let root = root_of(&a);
    let mut parts: Vec<&str> = tail(&a);
    parts.extend(tail(b));
    let out = format!("{root}{}", parts.join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// `Path(p).joinpath(*parts)`.
pub fn join_all<'a>(a: &str, parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = norm(a);
    for p in parts {
        out = join(&out, p);
    }
    out
}

/// `Path(p).parent` (a normalized path in, a normalized path out).
pub fn parent(p: &str) -> String {
    let p = norm(p);
    let root = root_of(&p).to_string();
    let t = tail(&p);
    if t.is_empty() {
        return p;
    }
    let out = format!("{root}{}", t[..t.len() - 1].join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// `Path(p).parents`, nearest first.
pub fn parents(p: &str) -> Vec<String> {
    let p = norm(p);
    let root = root_of(&p).to_string();
    let t = tail(&p);
    (0..t.len())
        .rev()
        .map(|n| {
            let s = format!("{root}{}", t[..n].join("/"));
            if s.is_empty() {
                ".".into()
            } else {
                s
            }
        })
        .collect()
}

/// `Path(p).name`.
pub fn name(p: &str) -> String {
    let p = norm(p);
    tail(&p).last().map(|s| s.to_string()).unwrap_or_default()
}

fn dot_index(n: &str) -> Option<usize> {
    let i = n.rfind('.')?;
    let chars_before = n[..i].chars().count();
    let total = n.chars().count();
    (chars_before > 0 && chars_before < total - 1).then_some(i)
}

/// `Path(p).suffix`.
pub fn suffix(p: &str) -> String {
    let n = name(p);
    dot_index(&n)
        .map(|i| n[i..].to_string())
        .unwrap_or_default()
}

/// `Path(p).stem`.
pub fn stem(p: &str) -> String {
    let n = name(p);
    match dot_index(&n) {
        Some(i) => n[..i].to_string(),
        None => n,
    }
}

/// `Path(p).with_name(n)`.
pub fn with_name(p: &str, n: &str) -> String {
    join(&parent(p), n)
}

/// `Path(p) == Path(q)` on POSIX.
pub fn same(p: &str, q: &str) -> bool {
    norm(p) == norm(q)
}

/// `Path(p).is_relative_to(root)`: the same root and `root`'s parts first.
pub fn is_relative_to(p: &str, root: &str) -> bool {
    let (p, r) = (norm(p), norm(root));
    if root_of(&p) != root_of(&r) {
        return false;
    }
    let (pt, rt) = (tail(&p), tail(&r));
    pt.len() >= rt.len() && pt[..rt.len()] == rt[..]
}

/// `Path(p).relative_to(root)` for a path known to be relative to it.
pub fn relative_to(p: &str, root: &str) -> String {
    let (p, r) = (norm(p), norm(root));
    let pt = tail(&p);
    let n = tail(&r).len();
    let s = pt[n..].join("/");
    if s.is_empty() {
        ".".into()
    } else {
        s
    }
}

/// `Path(p).expanduser()`, the owner's home given (`HOME`); `~user` through the password database.
pub fn expanduser(p: &str, home: &str) -> String {
    let p = norm(p);
    if is_abs(&p) {
        return p;
    }
    let t = tail(&p);
    let Some(first) = t.first() else { return p };
    if !first.starts_with('~') {
        return p;
    }
    let userhome = if *first == "~" {
        home.to_string()
    } else {
        match user_home(&first[1..]) {
            Some(h) => h,
            None => return p,
        }
    };
    let userhome = userhome.trim_end_matches('/');
    let expanded = if userhome.is_empty() {
        "/".to_string()
    } else {
        userhome.to_string()
    };
    join_all(&expanded, t[1..].iter().copied())
}

/// `os.path.expanduser(text)` (no pathlib normalization), the owner's home given.
pub fn os_expanduser(p: &str, home: &str) -> String {
    if !p.starts_with('~') {
        return p.to_string();
    }
    let i = p[1..].find('/').map(|i| i + 1).unwrap_or(p.len());
    let userhome = if i == 1 {
        home.to_string()
    } else {
        match user_home(&p[1..i]) {
            Some(h) => h,
            None => return p.to_string(),
        }
    };
    let out = format!("{}{}", userhome.trim_end_matches('/'), &p[i..]);
    if out.is_empty() {
        "/".into()
    } else {
        out
    }
}

fn user_home(user: &str) -> Option<String> {
    let c = CString::new(user).ok()?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call and `buf` outlives the read of `pw_dir` below.
    let rc =
        unsafe { libc::getpwnam_r(c.as_ptr(), &mut pwd, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: getpwnam_r filled `pw_dir` with a NUL-terminated string inside `buf`.
    let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
    Some(dir.to_string_lossy().into_owned())
}

/// `os.path.normpath`.
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let (root, rest) = split_root(p);
    let mut comps: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (root.is_empty() && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let out = format!("{root}{}", comps.join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// `os.path.splitext`.
pub fn splitext(p: &str) -> (String, String) {
    let sep = p.rfind('/').map(|i| i as isize).unwrap_or(-1);
    if let Some(dot) = p.rfind('.') {
        if dot as isize > sep {
            let mut i = (sep + 1) as usize;
            while i < dot {
                if p.as_bytes()[i] != b'.' {
                    return (p[..dot].to_string(), p[dot..].to_string());
                }
                i += 1;
            }
        }
    }
    (p.to_string(), String::new())
}

/// posixpath.join for two parts.
fn pjoin(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// posixpath.split.
fn psplit(p: &str) -> (String, String) {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (mut head, tail) = (p[..i].to_string(), p[i..].to_string());
    if !head.is_empty() && head.chars().any(|c| c != '/') {
        head = head.trim_end_matches('/').to_string();
    }
    (head, tail)
}

fn cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".into())
}

/// `os.path.abspath`.
pub fn abspath(p: &str) -> String {
    if is_abs(p) {
        normpath(p)
    } else {
        normpath(&pjoin(&cwd(), p))
    }
}

/// `os.path.realpath(p)` (non-strict, CPython 3.12 `_joinrealpath`): links followed as far as they go, `..` applied
/// after them, a missing part kept as written, a link loop left as written from the loop on.
pub fn realpath(p: &str) -> String {
    let mut seen = std::collections::HashMap::new();
    let (path, _) = join_real("", p, &mut seen);
    abspath(&path)
}

fn join_real(
    path: &str,
    rest: &str,
    seen: &mut std::collections::HashMap<String, Option<String>>,
) -> (String, bool) {
    let mut path = path.to_string();
    let mut rest = rest.to_string();
    if rest.starts_with('/') {
        rest = rest[1..].to_string();
        path = "/".into();
    }
    while !rest.is_empty() {
        let (name, after) = match rest.find('/') {
            Some(i) => (rest[..i].to_string(), rest[i + 1..].to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = after;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if !path.is_empty() {
                let (head, tail) = psplit(&path);
                path = head;
                if tail == ".." {
                    path = pjoin(&pjoin(&path, ".."), "..");
                }
            } else {
                path = "..".into();
            }
            continue;
        }
        let newpath = pjoin(&path, &name);
        let is_link = fs::symlink_metadata(&newpath)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(known) = seen.get(&newpath) {
            match known {
                Some(resolved) => {
                    path = resolved.clone();
                    continue;
                }
                None => return (pjoin(&newpath, &rest), false),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = match fs::read_link(&newpath) {
            Ok(t) => t.to_string_lossy().into_owned(),
            Err(_) => {
                path = newpath;
                continue;
            }
        };
        let (resolved, ok) = join_real(&path, &target, seen);
        if !ok {
            return (pjoin(&resolved, &rest), false);
        }
        path = resolved.clone();
        seen.insert(newpath, Some(resolved));
    }
    (path, true)
}

/// Python's `RuntimeError("Symlink loop from ...")` from `Path.resolve()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkLoop(pub String);

/// `Path(p).resolve()` (non-strict): `realpath`, then a stat to raise on a link loop.
pub fn resolve(p: &str) -> Result<String, SymlinkLoop> {
    let s = norm(&realpath(&norm(p)));
    if let Err(e) = fs::metadata(&s) {
        if e.raw_os_error() == Some(libc::ELOOP) {
            return Err(SymlinkLoop(s));
        }
    }
    Ok(s)
}

/// `os.path.lexists`.
pub fn lexists(p: &str) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// `Path(p).exists()` / `os.path.exists` (links followed; any error is "no").
pub fn exists(p: &str) -> bool {
    fs::metadata(p).is_ok()
}

/// `Path(p).is_dir()`.
pub fn is_dir(p: &str) -> bool {
    fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// `Path(p).is_file()`.
pub fn is_file(p: &str) -> bool {
    fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// `Path(p).is_symlink()`.
pub fn is_symlink(p: &str) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// A FIFO, socket or device: a "special" file.
pub fn is_special(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| {
            let t = m.file_type();
            t.is_fifo() || t.is_socket() || t.is_block_device() || t.is_char_device()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathlib_spelling() {
        assert_eq!(norm("a//b/./c/"), "a/b/c");
        assert_eq!(norm(""), ".");
        assert_eq!(norm("//x"), "//x");
        assert_eq!(norm("///x"), "/x");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("a"), ".");
        assert_eq!(suffix("a.tar.gz"), ".gz");
        assert_eq!(suffix(".bashrc"), "");
        assert_eq!(suffix("a."), "");
        assert_eq!(stem("a."), "a.");
        assert_eq!(splitext("a."), ("a".into(), ".".into()));
        assert_eq!(splitext("..a.b"), ("..a".into(), ".b".into()));
        assert_eq!(splitext(".bashrc"), (".bashrc".into(), String::new()));
        assert_eq!(normpath("/ws/notes/../a.md"), "/ws/a.md");
        assert_eq!(normpath("../a"), "../a");
        assert_eq!(parents("/a/b"), vec!["/a".to_string(), "/".to_string()]);
        assert!(is_relative_to("/a/b", "/a"));
        assert!(!is_relative_to("/ab", "/a"));
        assert_eq!(expanduser("~/x", "/h/"), "/h/x");
        assert_eq!(os_expanduser("~", "/h"), "/h");
        assert_eq!(join("/", "x/y"), "/x/y");
        assert_eq!(join("//", "x"), "//x");
        assert_eq!(join(".", "x"), "x");
        assert_eq!(join("a", "/b"), "/b");
    }
}
