//! The word and clause rules of `items.py` that `ferb-memory::terms` doesn't hold: where the owner's words state a
//! fact (`owner_clauses`, MEM-419/420), whether a quote is whole clauses (`whole_clauses`), the one quote check with
//! its clause form (`quoted`), chat capture (`capture`) and a fact's one-way fingerprint.

use ferb_memory::pystr;
use ferb_memory::terms;

use crate::py;

pub use terms::{contains_words, query_terms, slug, subject_key, weighted};

/// items.py:111: where a clause ends; a period ends one only where it ends a sentence (`sentence_end`, MEM-431).
const CLAUSE_MARKS: [char; 6] = [',', ';', ':', '!', '?', '\n'];
/// items.py:112.
const EDGE: &str = " \t.\"'“”‘’()[]";

/// policy/blocked.py: the refusals the owner reads (CAPTURE_TOLD, ORCH-519).
pub const NOT_IN_WORDS: &str = "the words I quoted from you aren't in what you wrote";
pub const DOESNT_SAY: &str = "the words I quoted from you don't say it";

/// items.py:188 `CAPTURE_TOLD`: each capture refusal in the owner's words.
pub fn capture_told(refused: &str) -> Option<&'static str> {
    match refused {
        "quote not in the owner's words" => Some(NOT_IN_WORDS),
        "the quote holds nothing of what would be saved" => Some(DOESNT_SAY),
        _ => None,
    }
}

fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c: char| chars.contains(c))
}

/// `re.finditer(r"\s+".join(map(re.escape, words)), text, re.IGNORECASE)` tried at one position.
fn match_at(text: &[char], at: usize, words: &[Vec<char>]) -> Option<usize> {
    let mut i = at;
    for (n, w) in words.iter().enumerate() {
        if n > 0 {
            let start = i;
            while i < text.len() && pystr::is_space(text[i]) {
                i += 1;
            }
            if i == start {
                return None;
            }
        }
        for &c in w {
            if i >= text.len() || !py::same_ignoring_case(text[i], c) {
                return None;
            }
            i += 1;
        }
    }
    Some(i)
}

/// items.py:115 `_found`: where `part` sits in `text` as whole words (case and spacing aside), in characters.
pub fn found(text: &str, part: &str) -> Option<(usize, usize)> {
    let edge = format!("{EDGE},;:!?");
    let words: Vec<Vec<char>> = pystr::split_ws(strip_chars(part, &edge))
        .map(|w| w.chars().collect())
        .collect();
    if words.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    while pos <= chars.len() {
        match match_at(&chars, pos, &words) {
            Some(end) => {
                let before = pos == 0 || !pystr::word_char(chars[pos - 1]);
                if before && (end == chars.len() || !pystr::word_char(chars[end])) {
                    return Some((pos, end));
                }
                pos = if end > pos { end } else { pos + 1 };
            }
            None => pos += 1,
        }
    }
    None
}

/// MEM-431: the period at `i` ends a sentence: space or the text's end follows it, and the word it closes isn't an
/// abbreviation by its shape. An abbreviation is single letters joined by periods ("e.g.", "U.S.") or a word of one
/// to four letters followed by a capitalised word or a number ("Dr. Rao", "prof. Zorblat", "no. 5"). A period inside
/// a word ("v2.1", "kartikey.xyz") is never followed by space, so it never ends one. Structure only, no word list.
fn sentence_end(chars: &[char], i: usize) -> bool {
    if chars[i] != '.' || chars.get(i + 1).is_some_and(|&c| !pystr::is_space(c)) {
        return false;
    }
    let from = chars[..i]
        .iter()
        .rposition(|&c| pystr::is_space(c))
        .map_or(0, |p| p + 1);
    let word: String = chars[from..i].iter().collect();
    if word.contains('.') {
        let initials = word
            .split('.')
            .all(|s| s.chars().count() == 1 && s.chars().all(char::is_alphabetic));
        return !initials;
    }
    let short = (1..=4).contains(&word.chars().count()) && word.chars().all(char::is_alphabetic);
    let next = chars[i + 1..].iter().find(|&&c| !pystr::is_space(c));
    !(short && next.is_some_and(|c| c.is_uppercase() || c.is_numeric()))
}

/// Where a clause ends: a clause mark, or a period that ends a sentence (MEM-431).
fn clause_bound(chars: &[char], i: usize) -> bool {
    CLAUSE_MARKS.contains(&chars[i]) || sentence_end(chars, i)
}

/// items.py:127 `_clause_span`, with a sentence's end as a bound too. Fixed against Python (MEM-431): Python never
/// cut at a period, so a fact quoted from a message ("roll no: 2024eb02238 . THEY SHOULDN'T …") was saved with every
/// sentence after it up to the next comma (live 2026-11-15).
fn clause_span(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    let left = (0..start)
        .rev()
        .find(|&i| clause_bound(chars, i))
        .map_or(0, |i| i + 1);
    let end = end.min(chars.len());
    let right = (end..chars.len())
        .find(|&i| clause_bound(chars, i))
        .unwrap_or(chars.len());
    (left, right)
}

/// items.py:133 `owner_clauses`: the clause(s) of the owner's `text` that hold `part` as whole words, in their own
/// spelling, or None.
pub fn owner_clauses(text: &str, part: &str) -> Option<String> {
    let (s, e) = found(text, part)?;
    let chars: Vec<char> = text.chars().collect();
    let (left, right) = clause_span(&chars, s, e);
    let span: String = chars[left..right].iter().collect();
    let out = strip_chars(&span, &format!("{EDGE}!?,;:"));
    (!out.is_empty()).then(|| out.to_owned())
}

/// items.py:147 `whole_clauses`.
pub fn whole_clauses(text: &str, part: &str) -> bool {
    let Some((s, e)) = found(text, part) else {
        return false;
    };
    let chars: Vec<char> = text.chars().collect();
    let (left, right) = clause_span(&chars, s, e);
    !chars[left..s]
        .iter()
        .chain(chars[e..right].iter())
        .any(|&c| pystr::word_char(c))
}

/// items.py:206 `owner_span_at`.
pub fn owner_span_at(text: &str, part: &str) -> Option<(usize, usize)> {
    found(text, part)
}

/// `re.split(r"\n[ \t]*\n", text)`.
fn split_blank_lines(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
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

/// items.py:157 `quoted`: every blank-line-separated part of `quote` is whole words (with `clause`, whole
/// clauses) of `owner_words`. Without `clause` it is `ferb-memory`'s one check, which also compares words
/// (MEM-430); a clause check (a forget's rule) stays Python's: it is about where the owner's clause ends.
pub fn quoted(owner_words: &str, quote: &str, clause: bool) -> bool {
    if !clause {
        return terms::quoted(owner_words, quote);
    }
    let parts: Vec<&str> = split_blank_lines(quote)
        .into_iter()
        .filter(|p| !pystr::strip(p).is_empty())
        .collect();
    !parts.is_empty() && parts.iter().all(|p| whole_clauses(owner_words, p))
}

/// items.py:179 `Capture`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    pub origin: String,
    pub content: String,
    pub refused: String,
}

/// items.py:192 `capture`.
pub fn capture(own_words: &str, content: &str, quote: &str) -> Capture {
    let content = pystr::strip(content).to_owned();
    if !quoted(own_words, quote, false) {
        return Capture {
            origin: String::new(),
            content,
            refused: "quote not in the owner's words".into(),
        };
    }
    let clause = owner_clauses(own_words, &content);
    if clause.is_none() {
        let q = query_terms(quote);
        let c = query_terms(&content);
        if !q.iter().any(|t| c.contains(t)) {
            return Capture {
                origin: String::new(),
                content,
                refused: "the quote holds nothing of what would be saved".into(),
            };
        }
    }
    match clause {
        Some(words) => Capture {
            origin: "owner_stated".into(),
            content: words,
            refused: String::new(),
        },
        None => Capture {
            origin: "inferred".into(),
            content,
            refused: String::new(),
        },
    }
}

/// items.py:250 `fingerprint`.
pub fn fingerprint(kind: &str, subject: &str, key: &str, content: &str) -> String {
    let norm: Vec<String> = [kind, subject, key, content]
        .iter()
        .map(|x| {
            pystr::split_ws(&pystr::lower(x))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    py::sha256_hex(&norm.join("|"))
}
