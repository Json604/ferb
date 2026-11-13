//! Amounts as a page writes them (`ext_bridge.py` `_AMOUNT`, `label_amount`, `page_amounts`;
//! decision: GATE-578): a currency sign or a common ISO 4217 code before or after a number. A shape,
//! never a meaning. Python's pattern
//!
//! ```text
//! CUR \s? \d[\d,]*(?:\.\d{1,2})?  |  \b \d[\d,]*(?:\.\d{1,2})? \s? CUR
//! CUR = [$€£¥₹₩₽₺₪₫฿₱₦₴] | Rs\.? | (?:USD|EUR|...)\b
//! ```
//!
//! is matched by hand with Python's `\d`, `\s` and `\w` (ferb-sanitize's Python-exact Unicode
//! classes). No branch ever needs to backtrack: after the greedy parts, what a shorter take leaves
//! next (a digit, a comma, a dot, a space) can't start what follows, so the greedy match is `re`'s.

use ferb_sanitize::parity::{is_alnum, is_decimal, is_space};

/// `re`'s `\w` on text: `str.isalnum()` or the underscore.
fn word_char(c: char) -> bool {
    c == '_' || is_alnum(c)
}

const SIGNS: &str = "$€£¥₹₩₽₺₪₫฿₱₦₴";
const CODES: [&str; 29] = [
    "USD", "EUR", "GBP", "INR", "JPY", "CNY", "AUD", "CAD", "CHF", "SGD", "HKD", "NZD", "SEK",
    "NOK", "DKK", "PLN", "BRL", "MXN", "ZAR", "AED", "SAR", "KRW", "TRY", "THB", "IDR", "MYR",
    "PHP", "VND", "ILS",
];
const LAST_CODE: &str = "RUB";
/// GATE-578: at most this many distinct amounts kept from one snapshot.
pub const PAGE_AMOUNTS: usize = 200;

fn at(s: &[char], i: usize) -> Option<char> {
    s.get(i).copied()
}

/// `CUR` at `i`: where it ends.
fn currency(s: &[char], i: usize) -> Option<usize> {
    let c = at(s, i)?;
    if SIGNS.contains(c) {
        return Some(i + 1);
    }
    if c == 'R' && at(s, i + 1) == Some('s') {
        return Some(if at(s, i + 2) == Some('.') {
            i + 3
        } else {
            i + 2
        });
    }
    let code: String = s.get(i..i + 3)?.iter().collect();
    if CODES.contains(&code.as_str()) || code == LAST_CODE {
        let end = i + 3;
        // `\b` after the code's last letter: the next character isn't a word character
        if at(s, end).is_none_or(|n| !word_char(n)) {
            return Some(end);
        }
    }
    None
}

/// `\d[\d,]*(?:\.\d{1,2})?` at `i`: where it ends.
fn number(s: &[char], i: usize) -> Option<usize> {
    if !at(s, i).is_some_and(is_decimal) {
        return None;
    }
    let mut j = i + 1;
    while at(s, j).is_some_and(|c| c == ',' || is_decimal(c)) {
        j += 1;
    }
    if at(s, j) == Some('.') && at(s, j + 1).is_some_and(is_decimal) {
        j += 2;
        if at(s, j).is_some_and(is_decimal) {
            j += 1;
        }
    }
    Some(j)
}

fn space(s: &[char], i: usize) -> usize {
    if at(s, i).is_some_and(is_space) {
        i + 1
    } else {
        i
    }
}

/// The match starting at `i`, if any (the first branch, then the second).
fn match_at(s: &[char], i: usize) -> Option<usize> {
    if let Some(after) = currency(s, i) {
        if let Some(end) = number(s, space(s, after)) {
            return Some(end);
        }
    }
    let boundary = i == 0 || !word_char(s[i - 1]);
    if boundary {
        if let Some(after) = number(s, i) {
            return currency(s, space(s, after));
        }
    }
    None
}

/// `_AMOUNT.finditer(text)`: each match's text.
pub fn amounts(text: &str) -> Vec<String> {
    let s: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        match match_at(&s, i) {
            Some(end) => {
                out.push(s[i..end].iter().collect());
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

/// `label_amount`: the one amount a control's label shows ("Pay ₹248.75" -> "₹248.75"), `None` when
/// it shows none or several.
pub fn label_amount(label: Option<&str>) -> Option<String> {
    let head = ferb_py::value::prefix(label.unwrap_or(""), 300);
    let mut found: Vec<String> = vec![];
    for m in amounts(head) {
        let m = ferb_py::value::strip(&m).to_string();
        if !found.contains(&m) {
            found.push(m);
        }
    }
    (found.len() == 1).then(|| found.remove(0))
}

/// `page_amounts`: every amount the page shows, as written (white space squeezed), in page order,
/// each once, at most [`PAGE_AMOUNTS`].
pub fn page_amounts<'a>(texts: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut found: Vec<String> = vec![];
    for text in texts {
        for m in amounts(text.unwrap_or("")) {
            let m = ferb_py::value::squeeze(&m);
            if !found.contains(&m) {
                found.push(m);
            }
            if found.len() >= PAGE_AMOUNTS {
                return found;
            }
        }
    }
    found
}
