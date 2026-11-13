//! Python's `str` methods this crate's text relies on, with Python 3.12's character classes (from ferb-sanitize's
//! tables). Offsets are bytes; Python's are code points, so a caller that counts uses `chars()`.

pub use ferb_py::text::{is_alnum, is_digit, is_space, lower};
pub use ferb_sanitize::parity::casefold;

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `str.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `str.rstrip(chars)`.
pub fn rstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// `str.strip(chars)`.
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `str.split()` with no argument: runs of whitespace, none empty.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// `" ".join(s.split())`.
pub fn squeeze(s: &str) -> String {
    split_ws(s).join(" ")
}

/// Whether `c` ends a line for `str.splitlines()`.
fn line_break(c: char) -> bool {
    matches!(
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
    )
}

/// `str.splitlines()` (no ends kept): "\r\n" is one break.
pub fn splitlines(s: &str) -> Vec<&str> {
    splitlines_keep(s)
        .into_iter()
        .map(|line| {
            let trimmed = line.strip_suffix("\r\n").unwrap_or(line);
            if trimmed.len() != line.len() {
                trimmed
            } else {
                match line.chars().last() {
                    Some(c) if line_break(c) => &line[..line.len() - c.len_utf8()],
                    _ => line,
                }
            }
        })
        .collect()
}

/// `str.splitlines(keepends=True)`.
pub fn splitlines_keep(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if line_break(c) {
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = iter.peek() {
                    iter.next();
                    end += 1;
                }
            }
            out.push(&s[start..end]);
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `str.isdigit()` of a whole string (non-empty, every character a digit).
pub fn isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_digit)
}

/// `len(s)` in code points.
pub fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s[:n]` in code points.
pub fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `s[n:]` in code points.
pub fn suffix_from(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

/// `s[a:b]` in code points (clamped as Python clamps).
pub fn slice(s: &str, a: usize, b: usize) -> &str {
    if b <= a {
        return "";
    }
    let start = s.char_indices().nth(a).map(|(i, _)| i).unwrap_or(s.len());
    let end = s.char_indices().nth(b).map(|(i, _)| i).unwrap_or(s.len());
    &s[start..end.max(start)]
}

/// The byte offset of code point `n` (the end when past it).
pub fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
}

/// The code-point offset of byte `b`.
pub fn char_at(s: &str, b: usize) -> usize {
    s[..b].chars().count()
}

/// `str.rsplit(sep, 1)[0]`.
pub fn rsplit_once_head<'a>(s: &'a str, sep: &str) -> &'a str {
    match s.rfind(sep) {
        Some(i) => &s[..i],
        None => s,
    }
}

/// `template.format(**named)` for templates whose only fields are `{name}` (no `{{`, no format specs): one pass,
/// so a value holding "{other}" stays as it is.
pub fn format_named(template: &str, named: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match named.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `unicodedata.decimal(c)`: a decimal digit's value (Unicode keeps each script's digits as runs of ten from zero).
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return c.to_digit(10);
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

/// `int(text)` of a string of decimal digits (no sign, no spaces): `None` when a digit isn't decimal (Python raises),
/// `Some(None)` when it is too large for a `usize`.
pub fn int_of_digits(text: &str) -> Option<Option<usize>> {
    let mut n: Option<usize> = Some(0);
    for c in text.chars() {
        let d = decimal_value(c)? as usize;
        n = n
            .and_then(|v| v.checked_mul(10))
            .and_then(|v| v.checked_add(d));
    }
    Some(n)
}

/// `repr(str)`.
pub fn repr_str(s: &str) -> String {
    ferb_context::pyvalue::str_repr(s)
}

/// `repr(list[str])`.
pub fn repr_strs<S: AsRef<str>>(items: &[S]) -> String {
    let inner: Vec<String> = items.iter().map(|s| repr_str(s.as_ref())).collect();
    format!("[{}]", inner.join(", "))
}

/// `sorted(set_of_str)` (code-point order).
pub fn sorted_set<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> Vec<String> {
    let set: std::collections::BTreeSet<String> =
        items.into_iter().map(|s| s.as_ref().to_string()).collect();
    set.into_iter().collect()
}

/// `str(value)` of a decoded JSON value, exactly (a list or dict as its `repr`).
pub fn str_of(value: &serde_json::Value) -> String {
    ferb_context::pyvalue::to_str(&ferb_context::Py::from_value(value))
}

/// Seconds since the epoch of an aware time (a naive one as UTC).
pub fn epoch_seconds(dt: &ferb_py::time::DateTime) -> f64 {
    let (y, m, d) = (i64::from(dt.year), i64::from(dt.month), i64::from(dt.day));
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let offset = dt
        .offset
        .map(|(s, us)| f64::from(s) + f64::from(us) / 1e6)
        .unwrap_or(0.0);
    (days * 86_400 + i64::from(dt.hour) * 3600 + i64::from(dt.minute) * 60 + i64::from(dt.second))
        as f64
        + f64::from(dt.microsecond) / 1e6
        - offset
}
