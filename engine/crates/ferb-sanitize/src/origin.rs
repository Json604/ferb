//! Port of ferb/core/origin.py: FERB's origin vocabulary: where a record's content came from. Never a judgment of
//! intent or content. Trust follows origin (AGENTS.md rule 9); a record holding several origins keeps them all, its
//! trust is the least trusted one, and an unknown label counts as outside.
//!
//! decision: GATE-515 (`brief`, `shown`), GATE-519 (`kind`, `kinds`).

use std::collections::BTreeSet;

use crate::pyurl;
use crate::unicode as u;

pub const OWNER: &str = "owner";
pub const FERB: &str = "ferb";
pub const PRIVATE: &str = "private";
pub const INFERRED: &str = "inferred";
pub const EXTERNAL: &str = "external";
pub const OUTSIDE_RANK: u8 = 4;

fn clean(text: &str) -> String {
    // origin.py:16: `" ".join(str(text).replace(",", " ").split())[:120]` (120 code points)
    let joined = u::squeeze(&text.replace(',', " "));
    joined[..u::forward(&joined, 0, 120)].to_string()
}

/// origin.py:20.
pub fn external(kind: &str, detail: &str) -> String {
    let (kind, detail) = (u::lower(&clean(kind)), clean(detail));
    if detail.is_empty() {
        format!("{EXTERNAL}:{kind}")
    } else {
        format!("{EXTERNAL}:{kind}:{detail}")
    }
}

/// origin.py:25.
pub fn is_external(o: &str) -> bool {
    o == EXTERNAL || o.starts_with("external:")
}

/// origin.py:29.
pub fn rank(o: &str) -> u8 {
    match o {
        OWNER => 0,
        FERB => 1,
        PRIVATE => 2,
        INFERRED => 3,
        _ => OUTSIDE_RANK,
    }
}

/// Python compares str by code point; Rust's `str` order is by UTF-8 bytes, which is the same order.
fn items<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> Vec<&'a str> {
    origins.into_iter().filter(|o| !o.is_empty()).collect()
}

/// origin.py:33.
pub fn least_trusted<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> String {
    items(origins)
        .into_iter()
        .max_by_key(|o| (rank(o), *o))
        .unwrap_or(FERB)
        .to_string()
}

/// origin.py:38: whether a record's stored origin is outside content: it holds labels and every one is outside.
pub fn stored_outside<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> bool {
    let found = items(origins);
    !found.is_empty() && found.iter().all(|o| rank(o) == OUTSIDE_RANK)
}

/// origin.py:46.
pub fn join<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> String {
    let set: BTreeSet<&str> = items(origins).into_iter().collect();
    set.into_iter().collect::<Vec<_>>().join(",")
}

/// origin.py:50: a label without its detail: `external:web:example.com` → `external:web`.
pub fn kind(o: &str) -> String {
    if is_external(o) {
        o.split(':').take(2).collect::<Vec<_>>().join(":")
    } else {
        o.to_string()
    }
}

/// origin.py:55. decision: GATE-519.
pub fn kinds<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> BTreeSet<String> {
    items(origins).into_iter().map(kind).collect()
}

/// origin.py:61: a record's origins as a model is shown them. decision: GATE-515.
pub fn brief<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> BTreeSet<String> {
    let found: BTreeSet<&str> = items(origins).into_iter().collect();
    let outside = found.iter().filter(|o| is_external(o)).count();
    found
        .into_iter()
        .map(|o| {
            if outside > 1 && is_external(o) {
                kind(o)
            } else {
                o.to_string()
            }
        })
        .collect()
}

/// origin.py:72: `brief`, joined as a label line.
pub fn shown<'a, I: IntoIterator<Item = &'a str>>(origins: I) -> String {
    brief(origins).into_iter().collect::<Vec<_>>().join(",")
}

/// origin.py:77.
pub fn split(text: Option<&str>) -> BTreeSet<String> {
    text.unwrap_or("")
        .split(',')
        .filter(|o| !o.is_empty())
        .map(str::to_string)
        .collect()
}

/// origin.py:81: a tool result's or context source's recorded origins. Fail-closed: content with no recorded
/// origin is unlabeled outside content.
pub fn recorded(text: Option<&str>, has_output: bool) -> BTreeSet<String> {
    let found = split(text);
    if found.is_empty() && has_output {
        return BTreeSet::from([external("unlabeled", "")]);
    }
    found
}

/// origin.py:89: one `ToolResult.read_sources` entry → its origin.
pub fn from_read_source(kind: &str, ident: &str) -> String {
    match kind {
        "file" | "app" | "owner_account" => PRIVATE.to_string(),
        "download" => external("download", ""),
        "web" => {
            // a page, or a host a shell command reached; a malformed URL must not break labeling
            let host = if ident.contains("://") {
                pyurl::hostname(ident).ok().flatten()
            } else {
                None
            };
            external(
                "web",
                host.as_deref().filter(|h| !h.is_empty()).unwrap_or(ident),
            )
        }
        "origin" => ident.to_string(), // a record's stored origin, passed through by FERB's readers
        _ => external(kind, ident),    // email, message, and any unknown kind: outside content
    }
}
