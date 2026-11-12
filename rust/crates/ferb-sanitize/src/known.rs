//! Port of ferb/sanitize/known.py: the known-value registry. Only FERB's own credentials are known by value (the
//! model providers' API keys, the shell proxy's tokens, FERB's sign-in code): no saved secret and no value the
//! owner types into a hidden input. Memory only, never persisted.
//!
//! decision: GATE-572 (owner), GATE-573.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::generated as g;
use crate::rx::{Rx, Span};
use crate::unicode as u;

pub const REDACTED: &str = "[redacted]";
/// known.py:22: a value this long or shorter never matches inside a longer number.
pub const SHORT: usize = 6;

static ID_BODY: Rx = Rx::new(&g::KNOWN_ID_BODY);

fn hex(n: u32, width: usize) -> String {
    format!("{n:0width$x}")
}

/// `json.dumps(value, ensure_ascii=…)[1:-1]`.
pub(crate) fn json_escape(value: &str, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{}", hex(c as u32, 4))),
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{}", hex(u32::from(*unit), 4)));
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// `value.encode("unicode_escape").decode()`.
fn unicode_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if (0x20..0x7f).contains(&cp) => out.push(c),
            _ if cp < 0x100 => out.push_str(&format!("\\x{}", hex(cp, 2))),
            _ if cp < 0x10000 => out.push_str(&format!("\\u{}", hex(cp, 4))),
            _ => out.push_str(&format!("\\U{}", hex(cp, 8))),
        }
    }
    out
}

/// `urllib.parse.quote(value, safe="")`, or `quote_plus` when `plus`.
fn url_quote(value: &str, plus: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~') {
            out.push(b as char);
        } else if plus && b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `html.escape(value)` (quotes too).
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// known.py:39: how a value reads once escaped, when that differs from it: inside JSON (both `ensure_ascii`
/// ways), in a repr, URL-encoded (path and form) and HTML-escaped; in Python's order, each once.
pub fn escaped_forms(value: &str) -> Vec<String> {
    let all = [
        json_escape(value, false),
        json_escape(value, true),
        unicode_escape(value),
        url_quote(value, false),
        url_quote(value, true),
        html_escape(value),
    ];
    let mut out: Vec<String> = Vec::new();
    for form in all {
        if form != value && !out.contains(&form) {
            out.push(form);
        }
    }
    out
}

/// known.py:49: where FERB's generated id bodies lie in `s`. In free text a body has an a-f letter; under FERB's
/// own id keys (`keyed`) any body counts.
pub fn id_bodies(s: &str, keyed: bool) -> Vec<Span> {
    ID_BODY
        .caps_all(s)
        .into_iter()
        .filter_map(|c| c[1])
        .filter(|&(a, b)| keyed || s[a..b].bytes().any(|x| (b'a'..=b'f').contains(&x)))
        .collect()
}

/// known.py:62: where a known value stands in `s`: never strictly inside an id body; a short value never inside
/// a longer number; otherwise exactly. `bodies`: `id_bodies(s)`, when already read.
pub fn sealed_spans(s: &str, value: &str, bodies: Option<&[Span]>) -> Vec<Span> {
    if value.is_empty() {
        return Vec::new();
    }
    let own;
    let bodies = match bodies {
        Some(b) => b,
        None => {
            own = id_bodies(s, false);
            &own
        }
    };
    let short = !u::longer_than(value, SHORT);
    let (first, last) = (
        value.chars().next().is_some_and(u::is_digit),
        value.chars().next_back().is_some_and(u::is_digit),
    );
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = s[from..].find(value) {
        let start = from + i;
        let end = start + value.len();
        from = crate::rx::next_char(s, start);
        // `bisect_right(bodies, (start, len(s) + 1)) - 1`: the last body starting at or before `start`.
        let i = bodies.partition_point(|&(a, _)| a <= start);
        if i > 0 && end <= bodies[i - 1].1 && (start, end) != bodies[i - 1] {
            continue;
        }
        if short
            && ((first && u::char_before(s, start).is_some_and(u::is_digit))
                || (last && u::char_at(s, end).is_some_and(u::is_digit)))
        {
            continue;
        }
        out.push((start, end));
    }
    out
}

/// known.py:100 `KnownValues`: the known values and their escaped forms (a sorted set: Python's set has no order
/// and nothing depends on one).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct KnownValues(BTreeSet<String>);

impl KnownValues {
    pub fn new<I: IntoIterator<Item = String>>(values: I) -> KnownValues {
        KnownValues(values.into_iter().collect())
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, value: &str) -> bool {
        self.0.contains(value)
    }

    /// known.py:110: whether any value occurs at all.
    pub fn anywhere(&self, s: &str) -> bool {
        self.0
            .iter()
            .any(|v| !v.is_empty() && s.contains(v.as_str()))
    }
}

impl std::fmt::Debug for KnownValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KnownValues({} values)", self.0.len()) // how many, never which
    }
}

/// known.py:83: every place one of `values` stands in `s` (`sealed_spans`), the ids read once for all of them.
/// The spans come value by value in the set's sorted order (Python's order is its set's; every reader sorts or
/// only asks whether there is one).
pub fn known_spans(s: &str, values: &KnownValues, keyed: bool) -> Vec<Span> {
    let present: Vec<&str> = values
        .iter()
        .filter(|v| !v.is_empty() && s.contains(v))
        .collect();
    if present.is_empty() {
        return Vec::new();
    }
    let bodies = id_bodies(s, keyed);
    present
        .into_iter()
        .flat_map(|v| sealed_spans(s, v, Some(&bodies)))
        .collect()
}

/// known.py:95: whether `s` still shows one of the known `values` where the redactor would hide it.
pub fn holds_sealed(s: &str, values: &KnownValues) -> bool {
    !known_spans(s, values, false).is_empty()
}

#[derive(Default)]
struct Held {
    values: BTreeSet<String>,
    forms: Option<Arc<KnownValues>>,
    version: u64,
}

/// known.py:114: the known values (`add`, `forget`). `version` changes with every change.
#[derive(Default)]
pub struct Registry {
    held: Mutex<Held>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// known.py:128: one of FERB's own credentials: known when it has 4 characters or more.
    /// decision: GATE-573.
    pub fn add(&self, value: &str) {
        let mut held = self.lock();
        if u::longer_than(value, 3) && !held.values.contains(value) {
            held.values.insert(value.to_string());
            held.forms = None;
            held.version += 1;
        }
    }

    /// known.py:136: how many values are known (never which).
    pub fn lasting(&self) -> usize {
        self.lock().values.len()
    }

    pub fn forget(&self, value: &str) {
        let mut held = self.lock();
        if held.values.remove(value) {
            held.forms = None;
            held.version += 1;
        }
    }

    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// known.py:146: every known value and each one's escaped forms.
    pub fn values(&self) -> Arc<KnownValues> {
        let mut held = self.lock();
        if let Some(forms) = &held.forms {
            return Arc::clone(forms);
        }
        let mut all: BTreeSet<String> = held.values.clone();
        for value in &held.values {
            all.extend(escaped_forms(value));
        }
        let forms = Arc::new(KnownValues(all));
        held.forms = Some(Arc::clone(&forms));
        forms
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Registry({} values)", self.lasting())
    }
}
