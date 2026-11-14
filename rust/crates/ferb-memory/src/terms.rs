//! What search and import need from `ferb/memory/items.py`, and no more: the word rules (`query_terms`,
//! `weighted`, `contains_words`, `quoted`), the key and subject slugs, the origin labels, and a fact as search reads
//! it ([`Fact`], `current` / `prompt_ready`). Saving, versions and forgetting are the items part's (not ported here).

use sha2::{Digest, Sha256};

use crate::pystr;
use crate::times::{self, DateTime};

/// items.py:48.
pub const KINDS: [&str; 5] = ["fact", "preference", "person", "project", "env"];

/// items.py:59: stamps are compared as text in SQL, so a bound there is this much wider (two days).
pub const SQL_SLACK_MICROS: i64 = 2 * 86_400 * 1_000_000;

/// items.py:51 `LABELS`, as `label(origin)` names them.
pub fn label(origin: &str) -> &str {
    match origin {
        "owner_stated" => "your words",
        "inferred" => "FERB's understanding",
        "observed" => "observed",
        "third_party" => "from outside content",
        other => other,
    }
}

/// `query_terms`: lowercased runs of letters, numbers, marks and private-use characters, each once, in order.
pub fn query_terms(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    let push = |t: &mut String, terms: &mut Vec<String>| {
        if !terms.contains(t) {
            terms.push(std::mem::take(t));
        } else {
            t.clear();
        }
    };
    for c in pystr::lower(text).chars() {
        if pystr::word_char(c) {
            current.push(c);
        } else if !current.is_empty() {
            push(&mut current, &mut terms);
        }
    }
    if !current.is_empty() {
        push(&mut current, &mut terms);
    }
    terms
}

/// `weighted`: the terms found in at most half the documents (at least 2), all of them under two documents.
pub fn weighted<E>(
    terms: &[String],
    total: i64,
    mut matches: impl FnMut(&str) -> Result<i64, E>,
) -> Result<Vec<String>, E> {
    let mut out = Vec::new();
    for t in terms {
        if total < 2 || (matches(t)? as f64) <= f64::max(2.0, total as f64 / 2.0) {
            out.push(t.clone());
        }
    }
    Ok(out)
}

fn spaced(text: &str) -> String {
    pystr::squeeze(&pystr::casefold(text))
}

const NEEDLE_EDGE: &[char] = &[
    ' ', '.', ',', ';', ':', '!', '?', '"', '\'', '“', '”', '‘', '’', '(', ')', '[', ']',
];

/// `contains_words`: `part` in `text` as whole words, case and spacing aside, its sentence punctuation aside.
pub fn contains_words(text: &str, part: &str) -> bool {
    let needle_full = spaced(part);
    let needle = needle_full.trim_matches(NEEDLE_EDGE);
    if needle.is_empty() {
        return false;
    }
    let hay = spaced(text);
    let mut from = 0;
    while let Some(found) = hay[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let before = hay[..start].chars().next_back();
        let after = hay[end..].chars().next();
        if !before.is_some_and(pystr::word_char) && !after.is_some_and(pystr::word_char) {
            return true;
        }
        from = start + hay[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// `quoted(owner_words, quote)` (clause=False), the one check that a quote is the owner's own words: every
/// blank-line-separated part of `quote` is whole words of `owner_words` (Python's character comparison) or, failing
/// that, [`said_as_words`]. What gets saved is never taken from the quote, so only acceptance is loosened.
pub fn quoted(owner_words: &str, quote: &str) -> bool {
    let parts = split_blank_lines(quote);
    let parts: Vec<&str> = parts
        .into_iter()
        .filter(|p| !pystr::strip(p).is_empty())
        .collect();
    !parts.is_empty()
        && parts
            .iter()
            .all(|p| contains_words(owner_words, p) || said_as_words(owner_words, p))
}

/// decision: MEM-430
/// One part of a quote is the owner's words when its words (runs of letters, numbers, marks and private-use
/// characters, as `query_terms` reads them), in order and unbroken, are words the owner typed (case aside): punctuation, quote styles, line breaks and stray marks a paste carries (a terminal's
/// "▎", ">") are not words. Python's `quoted` compares characters, so one pasted mark between two of the owner's
/// words refused the owner's own request (a plan part, ORCH-646; a remembered preference; a schedule's words).
pub fn said_as_words(owner_words: &str, part: &str) -> bool {
    fn words(text: &str) -> Vec<String> {
        text.split(|c: char| !pystr::word_char(c))
            .filter(|w| !w.is_empty())
            .map(pystr::casefold)
            .collect()
    }
    let part = words(part);
    !part.is_empty()
        && words(owner_words)
            .windows(part.len())
            .any(|run| run == part.as_slice())
}

/// `re.split(r"\n[ \t]*\n", text)`.
fn split_blank_lines(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            let mut j = i + 1;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            if j < b.len() && b[j] == b'\n' {
                out.push(&text[start..i]);
                start = j + 1;
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out.push(&text[start..]);
    out
}

/// `slug(text, n)`: runs of non-word characters as `_`, casefolded, trimmed; "note" when nothing is left; past
/// `n` characters, cut with a hash of the whole.
pub fn slug(text: &str, n: Option<usize>) -> String {
    let folded = pystr::casefold(text);
    let mut value = String::with_capacity(folded.len());
    let mut in_run = false;
    for c in folded.chars() {
        if pystr::re_word(c) {
            value.push(c);
            in_run = false;
        } else if !in_run {
            value.push('_');
            in_run = true;
        }
    }
    let value = value.trim_matches('_');
    let value = if value.is_empty() { "note" } else { value };
    if let Some(n) = n {
        if pystr::char_len(value) > n {
            let digest = hex(&Sha256::digest(value.as_bytes()));
            return format!(
                "{}_{}",
                pystr::prefix(value, n.saturating_sub(17)),
                &digest[..16]
            );
        }
    }
    value.to_owned()
}

/// `subject_key`: "owner" when empty; a `kind:name` subject keeps its kind and slugs its name.
pub fn subject_key(subject: &str) -> String {
    let subject = pystr::strip(subject);
    if subject.is_empty() {
        return "owner".to_owned();
    }
    match subject.split_once(':') {
        None => slug(subject, None),
        Some((head, rest)) if !pystr::strip(rest).is_empty() => {
            format!("{}:{}", slug(head, None), slug(rest, None))
        }
        Some((head, _)) => slug(head, None),
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A memory item as search reads it (`MemoryItem`'s fields that search, the import's lookups and `prompt_ready`
/// read). The items part owns the full item.
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    pub origin: String,
    pub status: String,
    pub valid_from: DateTime,
    pub valid_to: Option<DateTime>,
    pub set_aside: bool,
}

/// `items.current(item, now)`: no end, or an end still ahead.
pub fn current(item: &Fact, now: &DateTime) -> Result<bool, crate::pyval::Crash> {
    match &item.valid_to {
        None => Ok(true),
        Some(end) => Ok(times::compare(end, now)? == std::cmp::Ordering::Greater),
    }
}

/// `items.prompt_ready(item, now)`.
pub fn prompt_ready(item: &Fact, now: &DateTime) -> Result<bool, crate::pyval::Crash> {
    Ok(current(item, now)?
        && !item.set_aside
        && (item.status == "active" || (item.status == "proposed" && item.origin == "observed")))
}
