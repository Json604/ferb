//! The word and clause rules of `items.py` that `ferb-memory::terms` doesn't hold: where the owner's words state a
//! fact (`owner_clauses`, MEM-419/420), whether a quote is whole clauses (`whole_clauses`), the one quote check with
//! its clause form (`quoted`), chat capture (`capture`) and a fact's one-way fingerprint.

use ferb_memory::pystr;
use ferb_memory::terms;

use crate::py;

pub use terms::{contains_words, query_terms, slug, subject_key, weighted};

/// items.py:111: where a clause ends; never a period ("Dr. Rao").
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

/// items.py:127 `_clause_span`.
fn clause_span(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    let left = chars[..start]
        .iter()
        .rposition(|c| CLAUSE_MARKS.contains(c))
        .map_or(0, |i| i + 1);
    let right = chars[end.min(chars.len())..]
        .iter()
        .position(|c| CLAUSE_MARKS.contains(c))
        .map_or(chars.len(), |i| end + i);
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
/// clauses) of `owner_words`.
pub fn quoted(owner_words: &str, quote: &str, clause: bool) -> bool {
    let parts: Vec<&str> = split_blank_lines(quote)
        .into_iter()
        .filter(|p| !pystr::strip(p).is_empty())
        .collect();
    !parts.is_empty()
        && parts.iter().all(|p| {
            if clause {
                whole_clauses(owner_words, p)
            } else {
                contains_words(owner_words, p)
            }
        })
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
