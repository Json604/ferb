//! Python's `str` answers, from tables written by the Python build (`generated.rs`, Unicode 15.0): the port reads
//! text exactly as Python 3.12 does whatever Unicode version this Rust has. Offsets in this crate are bytes; where
//! Python counts code points (`len`, a slice by number), the helpers here count characters.

use crate::generated as g;

// The one copy, in ferb-py (rust/fixtures/py/make_unicode.py made its tables as gen_rust.py made these).
pub use ferb_py::text::{is_alnum, is_digit, is_space, lower};

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let cp = c as u32;
    table
        .binary_search_by(|&(a, b)| {
            if cp < a {
                std::cmp::Ordering::Greater
            } else if cp > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `str.isdecimal()` (and `\d` in a str pattern).
pub fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        in_table(g::DECIMAL, c)
    }
}

/// `str.isalpha()`.
pub fn is_alpha(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic()
    } else {
        in_table(g::ALPHA, c)
    }
}

/// known.py:31 `word_char`: a letter, number or mark, or a private-use character.
pub fn word_char(c: char) -> bool {
    in_table(g::WORD_CHAR, c)
}

/// untrusted.py:105: a character `as_data` writes as an escape.
pub fn is_escaped_category(c: char) -> bool {
    in_table(g::ESCAPED, c)
}

/// `str.isupper()`.
pub fn is_upper_str(s: &str) -> bool {
    let mut cased = false;
    for c in s.chars() {
        if in_table(g::LOWER_OR_TITLE, c) {
            return false;
        }
        if !cased && in_table(g::UPPER, c) {
            cased = true;
        }
    }
    cased
}

/// `str.casefold()`, from Python's own table.
pub fn casefold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match g::CASEFOLD.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
            Ok(i) => out.push_str(g::CASEFOLD[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}

/// `len(s)`: code points.
pub fn char_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

/// Whether `len(s) > n`, reading no more than it needs.
pub fn longer_than(s: &str, n: usize) -> bool {
    s.len() > n && (s.len() > n * 4 || s.chars().nth(n).is_some())
}

/// The byte offset `n` code points before `pos` (0 when the text starts sooner): `max(0, pos - n)`.
pub fn back(s: &str, pos: usize, n: usize) -> usize {
    if n == 0 {
        return pos;
    }
    s[..pos]
        .char_indices()
        .rev()
        .nth(n - 1)
        .map_or(0, |(i, _)| i)
}

/// The byte offset `n` code points after `pos` (the end when the text ends sooner): `min(len, pos + n)`.
pub fn forward(s: &str, pos: usize, n: usize) -> usize {
    match s[pos..].char_indices().nth(n) {
        Some((i, _)) => pos + i,
        None => s.len(),
    }
}

/// `s[-n:]`: the last `n` code points.
pub fn last_chars(s: &str, n: usize) -> &str {
    &s[back(s, s.len(), n)..]
}

/// The character ending at byte `pos` (`s[pos - 1]` in Python).
pub fn char_before(s: &str, pos: usize) -> Option<char> {
    s[..pos].chars().next_back()
}

/// The character starting at byte `pos` (`s[pos]` in Python).
pub fn char_at(s: &str, pos: usize) -> Option<char> {
    s[pos..].chars().next()
}

/// `s.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `s.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `s.split()`: the runs of characters that aren't whitespace.
pub fn split_ws(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_space).filter(|w| !w.is_empty())
}

/// `" ".join(s.split())`.
pub fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for w in split_ws(s) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// The spans of `re.finditer(r"\S+", s)`.
pub fn word_spans(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        if is_space(c) {
            if let Some(a) = start.take() {
                out.push((a, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(a) = start {
        out.push((a, s.len()));
    }
    out
}
