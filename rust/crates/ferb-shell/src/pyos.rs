//! Python 3.12's path and number pieces the shell's outputs rest on: `os.path.realpath` (non-strict, posixpath.py
//! :420-495), `os.path.normpath`, `str(Path(...))`, `str.isalnum`, `str.split()`/`strip()` and `int()` of a str or of
//! bytes in base 16. Paths are bytes (a macOS path need not be UTF-8); the profile text needs UTF-8 and refuses the
//! rest (see `sandbox`).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

fn bytes(p: &Path) -> &[u8] {
    p.as_os_str().as_bytes()
}

fn path(b: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(b))
}

/// posixpath.join(a, b).
fn join(a: &[u8], b: &[u8]) -> Vec<u8> {
    if b.starts_with(b"/") {
        return b.to_vec();
    }
    let mut out = a.to_vec();
    if out.is_empty() || out.ends_with(b"/") {
        out.extend_from_slice(b);
    } else {
        out.push(b'/');
        out.extend_from_slice(b);
    }
    out
}

/// posixpath.split(p).
fn split(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let i = p.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let (mut head, tail) = (p[..i].to_vec(), p[i..].to_vec());
    if !head.is_empty() && !head.iter().all(|&c| c == b'/') {
        while head.ends_with(b"/") {
            head.pop();
        }
    }
    (head, tail)
}

/// posixpath.normpath (posix's `_path_normpath`).
pub fn normpath(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let lead = p.iter().take_while(|&&c| c == b'/').count();
    let root: &[u8] = match lead {
        0 => b"",
        2 => b"//",
        _ => b"/",
    };
    let mut parts: Vec<&[u8]> = Vec::new();
    for comp in p.split(|&c| c == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp == b".." {
            if !parts.is_empty() && parts[parts.len() - 1] != b".." {
                parts.pop();
            } else if root.is_empty() {
                parts.push(comp);
            }
            continue;
        }
        parts.push(comp);
    }
    let mut out = root.to_vec();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out = b".".to_vec();
    }
    out
}

fn lstat_is_link(p: &[u8]) -> bool {
    std::fs::symlink_metadata(path(p)).is_ok_and(|m| m.file_type().is_symlink())
}

type Seen = HashMap<Vec<u8>, Option<Vec<u8>>>;

fn joinrealpath(mut base: Vec<u8>, rest: &[u8], seen: &mut Seen) -> (Vec<u8>, bool) {
    let mut rest = rest.to_vec();
    if rest.starts_with(b"/") {
        rest.remove(0);
        base = b"/".to_vec();
    }
    while !rest.is_empty() {
        let (name, after) = match rest.iter().position(|&c| c == b'/') {
            Some(i) => (rest[..i].to_vec(), rest[i + 1..].to_vec()),
            None => (rest.clone(), Vec::new()),
        };
        rest = after;
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            if !base.is_empty() {
                let (head, tail) = split(&base);
                base = head;
                if tail == b".." {
                    base = join(&join(&base, b".."), b"..");
                }
            } else {
                base = b"..".to_vec();
            }
            continue;
        }
        let newpath = join(&base, &name);
        if !lstat_is_link(&newpath) {
            base = newpath;
            continue;
        }
        if let Some(known) = seen.get(&newpath) {
            match known {
                Some(resolved) => {
                    base = resolved.clone();
                    continue;
                }
                None => return (join(&newpath, &rest), false),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = match std::fs::read_link(path(&newpath)) {
            Ok(t) => bytes(&t).to_vec(),
            // os.readlink raising is not ignored in Python either; a link that vanished reads as no link here
            Err(_) => {
                seen.remove(&newpath);
                base = newpath;
                continue;
            }
        };
        let (resolved, ok) = joinrealpath(base, &target, seen);
        if !ok {
            return (join(&resolved, &rest), false);
        }
        seen.insert(newpath, Some(resolved.clone()));
        base = resolved;
    }
    (base, true)
}

/// `os.path.realpath(p)` (strict=False): links followed by name, a missing part kept as written, normalized.
pub fn realpath(p: &Path) -> PathBuf {
    let (resolved, _) = joinrealpath(Vec::new(), bytes(p), &mut Seen::new());
    abspath(&resolved)
}

fn abspath(p: &[u8]) -> PathBuf {
    if p.starts_with(b"/") {
        return path(&normpath(p));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    path(&normpath(&join(bytes(&cwd), p)))
}

/// `str(Path(text))` on POSIX (ferb-core's `py_path`), for bytes.
pub fn py_path(p: &Path) -> PathBuf {
    let b = bytes(p);
    let lead = b.iter().take_while(|&&c| c == b'/').count();
    let root: &[u8] = match lead {
        0 => b"",
        2 => b"//",
        _ => b"/",
    };
    let parts: Vec<&[u8]> = b
        .split(|&c| c == b'/')
        .filter(|c| !c.is_empty() && *c != b".")
        .collect();
    let mut out = root.to_vec();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out = b".".to_vec();
    }
    path(&out)
}

/// `dict.fromkeys(items)` as a list: first occurrence kept, order kept.
pub fn dedupe<T: PartialEq + Clone>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// `str.isalnum()` for one character (Python 3.12's Unicode tables, through ferb-sanitize).
pub fn is_alnum(c: char) -> bool {
    ferb_py::text::is_alnum(c)
}

/// `str.isspace()` for one character.
pub fn is_space(c: char) -> bool {
    ferb_py::text::is_space(c)
}

/// `str.strip()` (no argument).
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.split()` (no argument).
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|w| !w.is_empty()).collect()
}

/// The value of a Unicode decimal digit (`int()` reads any of them): its place in its run of ten.
fn digit_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    if !ferb_py::unicode::is_decimal(c) {
        return None;
    }
    let mut start = c as u32;
    while start > 0 && char::from_u32(start - 1).is_some_and(ferb_py::unicode::is_decimal) {
        start -= 1;
    }
    Some((c as u32 - start) % 10)
}

/// `int(text)` for a str in base 10: surrounding whitespace, a sign, any Unicode decimal digits, single
/// underscores between digits. None where Python raises ValueError (or past i128).
pub fn int_str(text: &str) -> Option<i128> {
    let t = strip(text);
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut value: i128 = 0;
    let mut prev_digit = false;
    let mut any = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = digit_value(c)?;
        value = value.checked_mul(10)?.checked_add(d as i128)?;
        prev_digit = true;
        any = true;
    }
    if !any || !prev_digit {
        return None;
    }
    Some(if neg { -value } else { value })
}

/// `int(raw, 16)` for bytes: ASCII whitespace around, a sign, an optional 0x prefix, single underscores.
pub fn int_hex_bytes(raw: &[u8]) -> Option<i128> {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = raw.iter().position(|c| !ws(c)).unwrap_or(raw.len());
    let end = raw.iter().rposition(|c| !ws(c)).map_or(start, |e| e + 1);
    let t = &raw[start..end];
    let (neg, mut body) = match t.split_first() {
        Some((b'-', b)) => (true, b),
        Some((b'+', b)) => (false, b),
        _ => (false, t),
    };
    let mut prefixed = false;
    if body.len() >= 2 && body[0] == b'0' && (body[1] | 0x20) == b'x' {
        body = &body[2..];
        prefixed = true;
    }
    let mut value: i128 = 0;
    let mut prev_digit = prefixed; // "0x_1" is allowed: an underscore may follow the prefix
    let mut any = false;
    for &c in body {
        if c == b'_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        let d = (c as char).to_digit(16)?;
        value = value.checked_mul(16)?.checked_add(d as i128)?;
        prev_digit = true;
        any = true;
    }
    if !any || !prev_digit {
        return None;
    }
    Some(if neg { -value } else { value })
}

/// Python's `repr()` of a str whose characters are all at most U+00FF (an HTTP request line read as latin-1).
pub fn repr_latin1(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..=0xa0).contains(&(c as u32)) || c as u32 == 0xad => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (c as u32) > 0xff => {
                // not reached for latin-1 text; kept printable as Python would for most characters
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normpath_matches_posixpath() {
        for (a, b) in [
            ("", "."),
            ("/", "/"),
            ("//", "//"),
            ("///", "/"),
            ("/a/./b/../c/", "/a/c"),
            ("a/../..", ".."),
            ("/..", "/"),
            ("//a//b", "//a/b"),
        ] {
            assert_eq!(normpath(a.as_bytes()), b.as_bytes(), "{a}");
        }
    }

    #[test]
    fn int_reads_as_python() {
        assert_eq!(int_str(" 1_0 "), Some(10));
        assert_eq!(int_str("١٢"), Some(12));
        assert_eq!(int_str("1__0"), None);
        assert_eq!(int_str("_1"), None);
        assert_eq!(int_str(""), None);
        assert_eq!(int_str("-5"), Some(-5));
        assert_eq!(int_hex_bytes(b" 0x1f "), Some(31));
        assert_eq!(int_hex_bytes(b"zz"), None);
        assert_eq!(int_hex_bytes(b"2"), Some(2));
    }
}
