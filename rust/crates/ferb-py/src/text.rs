//! Python's `str` answers some ports need beyond `value`'s: `isspace`, `isdigit`, `isalnum`, `isprintable`,
//! `lower` (Python 3.12's tables, Unicode 15.0, written by rust/fixtures/py/make_unicode.py, so the port reads text
//! exactly as Python does whatever Unicode version this Rust has), `repr` of a `str` or `bytes`, and trace's
//! `shorten`. One copy for every crate: ferb-sanitize, ferb-context, ferb-trace and ferb-orchestrator re-export these.

use std::fmt::Write;

use crate::unicode_str as t;

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    let at = table.partition_point(|&(_, hi)| hi < cp);
    table.get(at).is_some_and(|&(lo, _)| lo <= cp)
}

/// `str.isspace()` (and `\s` in a str pattern).
pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        matches!(c as u32, 0x09..=0x0d | 0x1c..=0x20)
    } else {
        in_table(t::SPACE, c)
    }
}

/// `str.isdigit()`.
pub fn is_digit(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        in_table(t::DIGIT, c)
    }
}

/// `str.isalnum()`.
pub fn is_alnum(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric()
    } else {
        in_table(t::ALNUM, c)
    }
}

/// `str.isprintable()` for one character: what `repr` leaves as it is.
pub fn is_printable(c: char) -> bool {
    !in_table(t::NOT_PRINTABLE, c)
}

/// A character whose NFKC form holds one of "/?#@:" (urllib.parse's netloc check).
pub fn nfkc_delim(c: char) -> bool {
    in_table(t::NFKC_DELIMS, c)
}

/// `str.lower()`, from Python's own table, with CPython's rule for a capital sigma (Objects/unicodeobject.c
/// `handle_capital_sigma`): final after a cased letter and before none, case-ignorable characters skipped on both
/// sides.
pub fn lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else if c == '\u{3a3}' {
            let before = s[..i]
                .chars()
                .rev()
                .find(|&x| !in_table(t::CASE_IGNORABLE, x));
            let after = s[i + c.len_utf8()..]
                .chars()
                .find(|&x| !in_table(t::CASE_IGNORABLE, x));
            let last = before.is_some_and(|x| in_table(t::CASED, x))
                && !after.is_some_and(|x| in_table(t::CASED, x));
            out.push(if last { '\u{3c2}' } else { '\u{3c3}' });
        } else {
            match t::LOWER.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
                Ok(k) => out.push_str(t::LOWER[k].1),
                Err(_) => out.push(c),
            }
        }
    }
    out
}

/// `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_str_repr(s, &mut out);
    out
}

/// `repr(str)` written onto `out`.
pub fn write_str_repr(s: &str, out: &mut String) {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if cp < 0x20 || cp == 0x7f => {
                let _ = write!(out, "\\x{cp:02x}");
            }
            _ if cp < 0x7f || is_printable(c) => out.push(c),
            _ if cp < 0x100 => {
                let _ = write!(out, "\\x{cp:02x}");
            }
            _ if cp < 0x10000 => {
                let _ = write!(out, "\\u{cp:04x}");
            }
            _ => {
                let _ = write!(out, "\\U{cp:08x}");
            }
        }
    }
    out.push(quote);
}

/// `repr(bytes)`.
pub fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::with_capacity(b.len() + 3);
    out.push('b');
    out.push(quote as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            _ => {
                let _ = write!(out, "\\x{c:02x}");
            }
        }
    }
    out.push(quote as char);
    out
}

/// ferb/trace/lines.py `shorten(text, n, fold)`: a short form that never cuts a word (ORCH-601): whitespace folded
/// when `fold`; past `n` characters, cut after the last sentence end in the first half or more, else at the last
/// space, with "…".
pub fn shorten(text: &str, n: usize, fold: bool) -> String {
    let s = if fold {
        crate::value::squeeze(text)
    } else {
        text.to_owned()
    };
    if s.chars().count() <= n {
        return s;
    }
    let head: Vec<char> = s.chars().take(n.saturating_sub(1)).collect();
    // re.finditer(r"[.!?]\s", head): non-overlapping, so a match skips the space it took
    let mut last_end = None;
    let mut i = 0;
    while i + 1 < head.len() {
        if matches!(head[i], '.' | '!' | '?') && is_space(head[i + 1]) {
            last_end = Some(i);
            i += 2;
        } else {
            i += 1;
        }
    }
    if let Some(e) = last_end {
        if e + 1 >= n / 2 {
            let cut: String = head[..=e].iter().collect();
            return format!("{cut} …");
        }
    }
    match head.iter().rposition(|&c| is_space(c)) {
        Some(sp) if sp > 0 => {
            let cut: String = head[..sp].iter().collect();
            format!(
                "{}…",
                cut.trim_end_matches([' ', ',', ';', ':', '(', '—', '-', '\n', '\t'])
            )
        }
        _ => format!("{}…", head.iter().collect::<String>()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pythons_whitespace_is_one_set_everywhere() {
        // `value::is_py_space` (Rust's White_Space plus U+001C..U+001F) and Python's own table agree on every char
        for cp in 0..=0x10ffffu32 {
            if let Some(c) = char::from_u32(cp) {
                assert_eq!(is_space(c), crate::value::is_py_space(c), "U+{cp:04X}");
            }
        }
    }

    #[test]
    fn str_answers_are_pythons() {
        assert!(
            is_digit('²') && !crate::unicode::is_decimal('²'),
            "superscript two is a digit, not a decimal"
        );
        assert!(is_alnum('ß') && is_alnum('٣') && !is_alnum('_'));
        assert!(!is_printable('\u{a0}') && is_printable('é'));
        assert_eq!(lower("ΟΔΟΣ ΑΣ."), "οδος ας.", "final sigma");
        assert_eq!(lower("Σ"), "σ", "alone, not final");
        assert_eq!(lower("İ"), "i\u{307}");
        assert_eq!(str_repr("it's"), "\"it's\"");
        assert_eq!(
            str_repr("a\u{a0}\u{2028}\u{1f600}\x7f'\""),
            "'a\\xa0\\u2028\u{1f600}\\x7f\\'\"'"
        );
        assert_eq!(bytes_repr(b"a'\x00\xff"), "b\"a'\\x00\\xff\"");
    }

    #[test]
    fn shorten_never_cuts_a_word() {
        assert_eq!(shorten("short", 10, true), "short");
        assert_eq!(
            shorten("First sentence here. Second one goes on and on", 30, true),
            "First sentence here. …"
        );
        assert_eq!(shorten("one two three four five", 12, true), "one two…");
        assert_eq!(shorten("abcdefghijklmnop", 5, true), "abcd…");
        assert_eq!(shorten("a  b\n c", 10, true), "a b c");
        assert_eq!(shorten("a  b\n c", 10, false), "a  b\n c");
    }
}
