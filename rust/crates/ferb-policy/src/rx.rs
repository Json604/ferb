//! Python `re` patterns the policy runs through a regex engine, written with Python's own classes: `{W}` is `\w`
//! (`str.isalnum()` or "_"), `{ALNUM}` is `[^\W_]`, `{S}` is `\s` (`str.isspace()`), `{D}` is `\d`
//! (`str.isdecimal()`), each spelled out from Python's tables so the regex crates' Unicode data changes nothing.
//! `{B}` is Python's `\b`. Patterns with lookaround run on `fancy-regex`, the rest on `regex`; both give
//! leftmost-first matches as Python's backtracking does. Each is checked against Python by the fixtures.
//!
//! A fancy-regex run can stop where Python's `re` cannot (its backtracking limits); none of these patterns nests
//! quantifiers that could get there on real text. Such a stop reads as "no further match"
//! (rust/JOURNAL.md lists it).

use std::sync::OnceLock;

use crate::py::in_table;
use crate::tables as t;

fn ranges(table: &[(u32, u32)], extra: &str) -> String {
    let mut out = String::from(extra);
    for &(a, b) in table {
        if a == b {
            out.push_str(&format!("\\x{{{a:x}}}"));
        } else {
            out.push_str(&format!("\\x{{{a:x}}}-\\x{{{b:x}}}"));
        }
    }
    out
}

fn cached(cell: &'static OnceLock<String>, f: impl FnOnce() -> String) -> &'static str {
    cell.get_or_init(f)
}

/// A pattern with `{W}`, `{ALNUM}`, `{S}`, `{D}` and `{B}` spelled out.
pub(crate) fn expand(pattern: &str) -> String {
    static W: OnceLock<String> = OnceLock::new();
    static A: OnceLock<String> = OnceLock::new();
    static S: OnceLock<String> = OnceLock::new();
    static D: OnceLock<String> = OnceLock::new();
    let w = cached(&W, || ranges(t::ALNUM, "_"));
    let a = cached(&A, || ranges(t::ALNUM, ""));
    let s = cached(&S, || ranges(t::SPACE, ""));
    let d = cached(&D, || ranges(t::DECIMAL, ""));
    let b = format!("(?:(?<=[{w}])(?![{w}])|(?<![{w}])(?=[{w}]))");
    pattern
        .replace("{B}", &b)
        .replace("{W}", w)
        .replace("{ALNUM}", a)
        .replace("{S}", s)
        .replace("{D}", d)
}

pub(crate) fn fancy(pattern: &str) -> fancy_regex::Regex {
    fancy_regex::RegexBuilder::new(&expand(pattern))
        .backtrack_limit(usize::MAX)
        .build()
        .unwrap_or_else(|e| panic!("ferb-policy: pattern {pattern:?} does not compile: {e}"))
}

pub(crate) fn plain(pattern: &str) -> regex::Regex {
    regex::RegexBuilder::new(&expand(pattern))
        .size_limit(1 << 26)
        .build()
        .unwrap_or_else(|e| panic!("ferb-policy: pattern {pattern:?} does not compile: {e}"))
}

/// Every match of a fancy pattern, as its groups' byte spans (group 0 first).
pub(crate) fn fancy_all(re: &fancy_regex::Regex, text: &str) -> Vec<Vec<Option<(usize, usize)>>> {
    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let Ok(caps) = caps else {
            break;
        };
        out.push(
            (0..caps.len())
                .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                .collect(),
        );
    }
    out
}

/// The first match of a fancy pattern (`re.search`).
pub(crate) fn fancy_first(
    re: &fancy_regex::Regex,
    text: &str,
) -> Option<Vec<Option<(usize, usize)>>> {
    let caps = re.captures(text).ok()??;
    Some(
        (0..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect(),
    )
}

/// `\w`.
pub(crate) fn word(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric() || (!c.is_ascii() && in_table(t::ALNUM, c))
}

/// `[^\W_]`.
pub(crate) fn alnum(c: char) -> bool {
    c != '_' && word(c)
}

/// A pattern compiled once: `once!(fancy, "…")` or `once!(plain, "…")`.
macro_rules! once {
    (fancy, $pat:expr) => {{
        static RE: std::sync::OnceLock<$crate::rx::Compiled> = std::sync::OnceLock::new();
        RE.get_or_init(|| $crate::rx::Compiled::Fancy($crate::rx::fancy($pat)))
    }};
    (plain, $pat:expr) => {{
        static RE: std::sync::OnceLock<$crate::rx::Compiled> = std::sync::OnceLock::new();
        RE.get_or_init(|| $crate::rx::Compiled::Plain($crate::rx::plain($pat)))
    }};
}
pub(crate) use once;

pub(crate) enum Compiled {
    Fancy(fancy_regex::Regex),
    Plain(regex::Regex),
}

impl Compiled {
    /// Every match's groups (`finditer`).
    pub(crate) fn all(&self, text: &str) -> Vec<Vec<Option<(usize, usize)>>> {
        match self {
            Compiled::Fancy(re) => fancy_all(re, text),
            Compiled::Plain(re) => re
                .captures_iter(text)
                .map(|caps| {
                    (0..caps.len())
                        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                        .collect()
                })
                .collect(),
        }
    }

    /// Every match's text (group 0).
    pub(crate) fn texts<'a>(&self, text: &'a str) -> Vec<&'a str> {
        self.all(text)
            .into_iter()
            .filter_map(|g| g[0].map(|(a, b)| &text[a..b]))
            .collect()
    }

    /// `re.search`.
    pub(crate) fn first(&self, text: &str) -> Option<Vec<Option<(usize, usize)>>> {
        match self {
            Compiled::Fancy(re) => fancy_first(re, text),
            Compiled::Plain(re) => re.captures(text).map(|caps| {
                (0..caps.len())
                    .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
                    .collect()
            }),
        }
    }

    /// `re.fullmatch` (the pattern is written anchored, `\A(?:…)\z`).
    pub(crate) fn full(&self, text: &str) -> bool {
        self.first(text).is_some()
    }
}
