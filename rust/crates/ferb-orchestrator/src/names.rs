//! ferb/orchestrator/names.py: how a task, schedule, reminder or timer is named in a heading (T-1055, ORCH-542),
//! and loops.py's `short_label` it rests on. Structure only: no model, no renaming.
//!
//! decision: ORCH-542

use crate::py::{len, prefix, rsplit_once_head, rstrip_chars, squeeze};

pub const NAME_CHARS: usize = 40;
pub const NAME_LIMIT: usize = 60;

/// Where `re.split(r'(?<=[.;:!?])\s|,\s|\s[—–-]\s', words, maxsplit=1)` first matches, in a text whose whitespace is
/// single spaces (`" ".join(text.split())`): the byte offset of the cut.
fn first_clause_end(words: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = words.char_indices().collect();
    let space = crate::py::is_space;
    for i in 0..chars.len() {
        let c = chars[i].1;
        if i > 0 && matches!(chars[i - 1].1, '.' | ';' | ':' | '!' | '?') && space(c) {
            return Some(chars[i].0);
        }
        if c == ',' && chars.get(i + 1).is_some_and(|x| space(x.1)) {
            return Some(chars[i].0);
        }
        if space(c)
            && chars
                .get(i + 1)
                .is_some_and(|x| matches!(x.1, '—' | '–' | '-'))
            && chars.get(i + 2).is_some_and(|x| space(x.1))
        {
            return Some(chars[i].0);
        }
    }
    None
}

/// loops.py:147 `short_label`: a goal's first clause, at most `limit` characters, cut at a word with "…".
pub fn short_label(text: &str, limit: usize) -> String {
    let words = squeeze(text);
    let first = match first_clause_end(&words) {
        Some(at) => &words[..at],
        None => words.as_str(),
    };
    let first = rstrip_chars(first, " .,;:!?");
    if len(first) <= limit {
        return first.to_string();
    }
    let cut = rstrip_chars(
        rsplit_once_head(prefix(first, limit.saturating_sub(1)), " "),
        " .,;:!?(",
    );
    format!("{cut}…")
}

/// names.py:13 `shown_name`.
pub fn shown_name(name: Option<&str>, text: &str) -> String {
    let words = squeeze(name.unwrap_or(""));
    if words.is_empty() {
        return short_label(text, NAME_CHARS);
    }
    if len(&words) <= NAME_LIMIT {
        return words;
    }
    let cut = rstrip_chars(
        rsplit_once_head(prefix(&words, NAME_LIMIT - 1), " "),
        " .,;:!?(",
    );
    format!("{cut}…")
}
