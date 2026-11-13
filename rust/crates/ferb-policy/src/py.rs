//! Python 3.12's `str` and `unicodedata` answers, from tables the Python build wrote (`tables.rs`, Unicode 15.0), so
//! the policy reads text exactly as `ferb/policy` does whatever Unicode version this Rust has. Offsets are bytes
//! here; where Python counts code points (`len`, a slice by number) the helpers count characters.

use crate::tables as t;

pub(crate) fn in_table(table: &[(u32, u32)], c: char) -> bool {
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

/// An idnadata table: (start, end exclusive).
pub(crate) fn in_intranges(table: &[(u32, u32)], cp: u32) -> bool {
    table
        .binary_search_by(|&(a, b)| {
            if cp < a {
                std::cmp::Ordering::Greater
            } else if cp >= b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn valued<T: Copy>(table: &[(u32, u32, T)], c: char) -> Option<T> {
    let cp = c as u32;
    table
        .binary_search_by(|&(a, b, _)| {
            if cp < a {
                std::cmp::Ordering::Greater
            } else if cp > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .ok()
        .map(|i| table[i].2)
}

/// `c.isspace()` (`\s`).
pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        matches!(c as u32, 0x09..=0x0d | 0x1c..=0x20)
    } else {
        in_table(t::SPACE, c)
    }
}

/// `c.isdecimal()` (`\d`).
pub fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        in_table(t::DECIMAL, c)
    }
}

/// `unicodedata.decimal(c)` for a decimal digit.
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    let cp = c as u32;
    let i = t::DECIMAL
        .partition_point(|&(a, _)| a <= cp)
        .checked_sub(1)?;
    let (a, b) = t::DECIMAL[i];
    (cp <= b).then_some((cp - a) % 10)
}

/// A run of decimal digits as ASCII digits (Python's `int()` and `float()` read any decimal digit).
pub fn ascii_digits(s: &str) -> String {
    s.chars()
        .map(|c| match decimal_value(c) {
            Some(v) => char::from_digit(v, 10).unwrap_or(c),
            None => c,
        })
        .collect()
}

/// `c.isdigit()`.
pub fn is_digit(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_digit()
    } else {
        in_table(t::DIGIT, c)
    }
}

/// `c.isalpha()`.
pub fn is_alpha(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic()
    } else {
        in_table(t::ALPHA, c)
    }
}

/// `c.isalnum()`.
pub fn is_alnum(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric()
    } else {
        in_table(t::ALNUM, c)
    }
}

/// `\w` in a str pattern: `c.isalnum()` or "_".
pub fn is_word(c: char) -> bool {
    c == '_' || is_alnum(c)
}

/// `c.isupper()` for one character.
pub fn is_upper(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_uppercase()
    } else {
        in_table(t::UPPER, c)
    }
}

/// `c.isprintable()`.
pub fn is_printable(c: char) -> bool {
    if c.is_ascii() {
        (' '..='~').contains(&c)
    } else {
        !in_table(t::NOT_PRINTABLE, c)
    }
}

/// `unicodedata.category(c)[0] == "M"`.
pub fn is_mark(c: char) -> bool {
    !c.is_ascii() && in_table(t::MARK, c)
}

/// `unicodedata.category(c) == "Mn"`.
pub fn is_mn(c: char) -> bool {
    !c.is_ascii() && in_table(t::MN, c)
}

/// `unicodedata.category(c) == "Cf"`.
pub fn is_cf(c: char) -> bool {
    !c.is_ascii() && in_table(t::CF, c)
}

/// `unicodedata.category(c) == "Sc"`.
pub fn is_sc(c: char) -> bool {
    in_table(t::SC, c)
}

/// `unicodedata.category(c)[0] in "LN"`.
pub fn is_letter_or_number(c: char) -> bool {
    in_table(t::LN, c)
}

/// provenance.py:38 `_word`: category L*, N*, M* or Co.
pub fn word_char(c: char) -> bool {
    in_table(t::LN, c) || in_table(t::MARK, c) || in_table(t::CO, c)
}

/// Whether `unicodedata.name(c)` answers.
pub fn has_name(c: char) -> bool {
    !in_table(t::NO_NAME, c)
}

/// `unicodedata.bidirectional(c)`: "" for none.
pub fn bidirectional(c: char) -> &'static str {
    valued(t::BIDI, c).unwrap_or("")
}

/// `unicodedata.combining(c)`.
pub fn combining(c: char) -> u8 {
    if (c as u32) < 0x300 {
        return 0;
    }
    valued(t::CCC, c).unwrap_or(0)
}

/// `s.lower()` (with CPython's final-sigma rule).
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

/// `s.casefold()`.
pub fn casefold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
            continue;
        }
        match t::CASEFOLD.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
            Ok(i) => out.push_str(t::CASEFOLD[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}

/// `s.split()`: runs of whitespace, none empty.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// `" ".join(s.split())`.
pub fn squeeze(s: &str) -> String {
    split_ws(s).join(" ")
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

/// `s.strip(chars)`.
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `s.rstrip(chars)`.
pub fn rstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// `s.lstrip(chars)`.
pub fn lstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}

/// `len(s)`.
pub fn char_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

/// `s[:n]` (code points).
pub fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `s[n:]` (code points).
pub fn after(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

/// `s.isdigit()` for a whole string.
pub fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_digit)
}

/// `s.isalnum()` for a whole string.
pub fn all_alnum(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_alnum)
}

/// `s.isprintable()` for a whole string.
pub fn all_printable(s: &str) -> bool {
    s.chars().all(is_printable)
}

/// `s.partition(sep)`.
pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, bool, &'a str) {
    match s.find(sep) {
        Some(i) => (&s[..i], true, &s[i + sep.len()..]),
        None => (s, false, ""),
    }
}

/// `s.rpartition(sep)`.
pub fn rpartition<'a>(s: &'a str, sep: &str) -> (&'a str, bool, &'a str) {
    match s.rfind(sep) {
        Some(i) => (&s[..i], true, &s[i + sep.len()..]),
        None => ("", false, s),
    }
}

/// Python's `re.I` comparison of a text character with an ASCII letter of a pattern (sre's simple case mapping and
/// its fixes: "İ" and "ı" are "i", "ſ" is "s", the Kelvin sign is "k").
pub fn ci_eq(c: char, letter: char) -> bool {
    if c.is_ascii() && c.eq_ignore_ascii_case(&letter) {
        return true;
    }
    matches!(
        (letter.to_ascii_lowercase(), c),
        ('i', '\u{130}') | ('i', '\u{131}') | ('s', '\u{17f}') | ('k', '\u{212a}')
    )
}

/// `[a-z]` under `re.I`.
pub fn ci_ascii_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
}

/// Python's `repr()` of a str (as `{label!r}` writes it).
pub fn repr(s: &str) -> String {
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
            c if is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `s.capitalize()`: the first character in title case, the rest lowered.
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let head = match t::CAPITALIZE.binary_search_by_key(&(first as u32), |&(cp, _)| cp) {
        Ok(i) => t::CAPITALIZE[i].1.to_string(),
        Err(_) => first.to_string(),
    };
    // the rest lowered in the whole text's context (a capital sigma's rule looks back past the first character)
    let whole = lower(s);
    let skip = lower(&first.to_string()).len();
    let _ = chars;
    head + &whole[skip..]
}

/// `s.upper()`.
pub fn upper(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_uppercase());
            continue;
        }
        match t::UPPER_MAP.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
            Ok(i) => out.push_str(t::UPPER_MAP[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}
