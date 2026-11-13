//! Small pieces of Python's own behavior the ported code relies on: `str.strip`, `str.splitlines`, `str.split()`,
//! `shlex.quote`, `Path.resolve()` (non-strict), `os.strerror`, slicing by characters.

use std::path::{Component, Path, PathBuf};

pub use ferb_py::value::strip;

/// `str.splitlines()`: every line boundary Python knows, the boundary dropped.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if boundary {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `str.split()` with no separator.
pub fn split_whitespace(text: &str) -> Vec<&str> {
    text.split(ferb_py::value::is_py_space)
        .filter(|w| !w.is_empty())
        .collect()
}

/// `text[-n:]` by characters.
pub fn last_chars(text: &str, n: usize) -> &str {
    let count = text.chars().count();
    if count <= n {
        return text;
    }
    let skip = count - n;
    let at = text
        .char_indices()
        .nth(skip)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    &text[at..]
}

/// `shlex.quote(s)`.
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if safe {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `Path.resolve()` (non-strict): symlinks followed as far as the path exists, the rest appended as written,
/// `..` and `.` resolved.
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    // the parts still to walk, last first
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    push_parts(&mut rest, &absolute);
    let mut done = PathBuf::from("/");
    let mut hops = 0;
    while let Some(part) = rest.pop() {
        if part == "." {
            continue;
        }
        if part == ".." {
            done.pop();
            continue;
        }
        let next = done.join(&part);
        match std::fs::read_link(&next) {
            Ok(target) if hops < 64 => {
                hops += 1;
                if target.is_absolute() {
                    done = PathBuf::from("/");
                }
                push_parts(&mut rest, &target);
            }
            _ => done = next,
        }
    }
    done
}

/// Pushes `path`'s parts onto `stack` so that its first part is popped first.
fn push_parts(stack: &mut Vec<std::ffi::OsString>, path: &Path) {
    let parts: Vec<std::ffi::OsString> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_os_string()),
            Component::ParentDir => Some("..".into()),
            Component::CurDir => Some(".".into()),
            _ => None,
        })
        .collect();
    stack.extend(parts.into_iter().rev());
}

/// `os.strerror(errno)`: the C library's words for an error number.
pub fn strerror(code: i32) -> String {
    // SAFETY: strerror returns a pointer to a static, NUL-terminated message for any value; it is copied at once.
    unsafe {
        let p = libc::strerror(code);
        if p.is_null() {
            return format!("Unknown error {code}");
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// `e.strerror or str(e)` for an I/O error.
pub fn os_words(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(code) => strerror(code),
        None => e.to_string(),
    }
}

/// `str(OSError)`: `[Errno N] words: 'path'` when a path is known, `[Errno N] words` otherwise.
pub fn os_error_text(e: &std::io::Error, path: Option<&Path>) -> String {
    match (e.raw_os_error(), path) {
        (Some(code), Some(p)) => format!(
            "[Errno {code}] {}: {}",
            strerror(code),
            py_repr_str(&p.to_string_lossy())
        ),
        (Some(code), None) => format!("[Errno {code}] {}", strerror(code)),
        (None, _) => e.to_string(),
    }
}

/// `repr(str)` for the plain strings FERB shows (paths, words): single quotes unless the text holds one and no
/// double quote.
pub fn py_repr_str(s: &str) -> String {
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
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
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
    fn splitlines_and_quote_follow_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd"), vec!["a", "b", "c", "", "d"]);
        assert_eq!(splitlines("a\n"), vec!["a"]);
        assert!(splitlines("").is_empty());
        assert_eq!(shlex_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shlex_quote("/a/b-c_d.e"), "/a/b-c_d.e");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(last_chars("héllo", 3), "llo");
        assert_eq!(py_repr_str("a'b"), "\"a'b\"");
    }

    #[test]
    fn resolve_follows_links_and_keeps_missing_tails() {
        let dir = tempfile::tempdir().unwrap();
        let real = resolve(dir.path());
        std::fs::create_dir(real.join("target")).unwrap();
        std::os::unix::fs::symlink(real.join("target"), real.join("link")).unwrap();
        assert_eq!(
            resolve(&real.join("link/missing/x")),
            real.join("target/missing/x")
        );
        assert_eq!(resolve(&real.join("target/../link")), real.join("target"));
    }
}
