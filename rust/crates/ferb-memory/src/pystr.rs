//! Python 3.12's answers about text, from the tables `rust/fixtures/memory/gen_rust.py` wrote out of Python itself
//! (`generated.rs`, Unicode 15.0), so this crate reads text as the Python build does whatever Unicode version Rust
//! has: `str.isspace/lower/casefold/isprintable/isdecimal`, `unicodedata.category`, `normalize("NFD")`, `split`,
//! `strip`, `repr`. Offsets are bytes; where Python counts code points, the helpers count characters.

use crate::generated as g;

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

/// `c.isspace()` (and `\s` in a str pattern).
pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        matches!(c as u32, 0x09..=0x0d | 0x1c..=0x20)
    } else {
        in_table(g::SPACE, c)
    }
}

/// `unicodedata.category(c).startswith("C")`.
pub fn is_other(c: char) -> bool {
    in_table(g::CAT_C, c)
}

/// `unicodedata.category(c).startswith("P")`.
pub fn is_punctuation_category(c: char) -> bool {
    in_table(g::CAT_P, c)
}

/// `unicodedata.category(c) == "Mn"`.
pub fn is_nonspacing_mark(c: char) -> bool {
    in_table(g::CAT_MN, c)
}

/// Category L*, N*, M* or Co: `items.query_terms`' word characters and `redact.word_char`.
pub fn word_char(c: char) -> bool {
    in_table(g::WORD, c)
}

/// `\w` in a str pattern.
pub fn re_word(c: char) -> bool {
    in_table(g::RE_WORD, c)
}

/// `c.isprintable()`.
pub fn is_printable(c: char) -> bool {
    !in_table(g::NOT_PRINTABLE, c)
}

/// `unicodedata.decimal(c)` for a character `c.isdecimal()` accepts (`\d`), else `None`.
pub fn decimal(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    let cp = c as u32;
    let at = g::DECIMAL.partition_point(|&(a, _)| a <= cp);
    let (a, b) = *g::DECIMAL.get(at.checked_sub(1)?)?;
    (a..=b).contains(&cp).then_some(cp - a)
}

/// `str.lower()`, with CPython's rule for a capital sigma (final after a cased letter and before none,
/// case-ignorable characters skipped on both sides).
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
                .find(|&x| !in_table(g::CASE_IGNORABLE, x));
            let after = s[i + c.len_utf8()..]
                .chars()
                .find(|&x| !in_table(g::CASE_IGNORABLE, x));
            let last = before.is_some_and(|x| in_table(g::CASED, x))
                && !after.is_some_and(|x| in_table(g::CASED, x));
            out.push(if last { '\u{3c2}' } else { '\u{3c3}' });
        } else {
            match g::LOWER.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
                Ok(k) => out.push_str(g::LOWER[k].1),
                Err(_) => out.push(c),
            }
        }
    }
    out
}

/// `str.casefold()`.
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

/// `unicodedata.combining(c)`.
pub fn combining(c: char) -> u8 {
    match g::CCC.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
        Ok(i) => g::CCC[i].1,
        Err(_) => 0,
    }
}

/// `unicodedata.normalize("NFD", s)`: each character's full canonical decomposition (a Hangul syllable by rule),
/// then every run of combining characters stably sorted by class.
pub fn nfd(s: &str) -> String {
    let mut chars: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if (0xac00..0xac00 + 11172).contains(&cp) {
            let n = cp - 0xac00;
            let push = |chars: &mut Vec<char>, v: u32| chars.extend(char::from_u32(v));
            push(&mut chars, 0x1100 + n / 588);
            push(&mut chars, 0x1161 + (n % 588) / 28);
            if !n.is_multiple_of(28) {
                push(&mut chars, 0x11a7 + n % 28);
            }
        } else if let Ok(i) = g::NFD.binary_search_by_key(&cp, |&(x, _)| x) {
            chars.extend(g::NFD[i].1.chars());
        } else {
            chars.push(c);
        }
    }
    let mut i = 0;
    while i < chars.len() {
        if combining(chars[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && combining(chars[i]) != 0 {
            i += 1;
        }
        chars[start..i].sort_by_key(|&c| combining(c));
    }
    chars.into_iter().collect()
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

/// `s.split()`.
pub fn split_ws(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_space).filter(|w| !w.is_empty())
}

/// `" ".join(s.split())`.
pub fn squeeze(s: &str) -> String {
    split_ws(s).collect::<Vec<_>>().join(" ")
}

/// `len(s)`.
pub fn char_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

/// The byte offset of character `n` (the end when the text is shorter).
pub fn byte_at(s: &str, n: usize) -> usize {
    if s.is_ascii() {
        return n.min(s.len());
    }
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// `s[:n]`.
pub fn prefix(s: &str, n: usize) -> &str {
    &s[..byte_at(s, n)]
}

/// `s[a:b]` with `0 <= a <= b`.
pub fn slice(s: &str, a: usize, b: usize) -> &str {
    let start = byte_at(s, a);
    let end = start + byte_at(&s[start..], b.saturating_sub(a));
    &s[start..end]
}

/// `repr(s)`.
pub fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
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
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_ascii() || is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}
