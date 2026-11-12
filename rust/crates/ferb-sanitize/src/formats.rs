//! Port of ferb/sanitize/formats.py: the secret formats a value shows by its own shape (vendor keys and tokens,
//! private keys, signed tokens, credential headers, URL credentials, database passwords on a command line, payment
//! cards with their expiry and CVV). One table, each entry named. Never a judgment of intent (AGENTS.md rule 4).
//!
//! decision: GATE-206, GATE-212 (cards, expiry, CVV), GATE-501 (the named table).
//! Offsets are bytes (Python's are code points); every count Python makes in code points is made in characters.

use std::collections::BTreeSet;

use crate::generated as g;
use crate::rx::{Rx, Span};
use crate::unicode as u;

/// formats.py:13.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
}

/// formats.py:10 `_H`: any whitespace but a line break.
pub fn is_h(c: char) -> bool {
    matches!(c as u32,
        0x09 | 0x0b | 0x0c | 0x1c..=0x1f | 0x20 | 0x85 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f
        | 0x205f | 0x3000)
}

struct Entry {
    format: &'static g::Format,
    rx: Rx,
}

fn table() -> &'static [Entry] {
    static T: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        g::FORMAT_TABLE
            .iter()
            .map(|format| Entry {
                format,
                rx: Rx::new(&format.pat),
            })
            .collect()
    })
}

const DISCORD: &str = "Discord bot token";

/// formats.py:60 `SECRET_PATTERNS`' names, in the table's order (Discord's included).
pub fn format_kinds() -> Vec<&'static str> {
    g::FORMAT_TABLE.iter().map(|f| f.kind).collect()
}

static PRIVATE_KEY_START: Rx = Rx::new(&g::FORMATS_PRIVATE_KEY_START);
static PRIVATE_KEY_END: Rx = Rx::new(&g::FORMATS_PRIVATE_KEY_END);
static KEY_BODY: Rx = Rx::new(&g::FORMATS_KEY_BODY);
static JWT_START: Rx = Rx::new(&g::FORMATS_JWT_START);
static JWT: Rx = Rx::new(&g::FORMATS_JWT);
static COOKIE: Rx = Rx::new(&g::FORMATS_COOKIE);
static QUOTED_DQ: Rx = Rx::new(&g::FORMATS_QUOTED_DQ);
static QUOTED_SQ: Rx = Rx::new(&g::FORMATS_QUOTED_SQ);
static MYSQL: Rx = Rx::new(&g::FORMATS_MYSQL);
static MYSQL_P: Rx = Rx::new(&g::FORMATS_MYSQL_P);
static URL: Rx = Rx::new(&g::FORMATS_URL);
static URL_START: Rx = Rx::new(&g::FORMATS_URL_START);
static PARAM: Rx = Rx::new(&g::FORMATS_PARAM);
static TOKEN_LIKE: Rx = Rx::new(&g::FORMATS_TOKEN_LIKE);
static DIGIT_RUN: Rx = Rx::new(&g::FORMATS_DIGIT_RUN);
static AFTER_CARD: Rx = Rx::new(&g::FORMATS_AFTER_CARD);

/// `PRIVATE_KEY_START.finditer(s)` / `PRIVATE_KEY_END.search(s, pos)` for the stream (stream.py:161-162).
pub(crate) fn private_key_starts(s: &str) -> Vec<Span> {
    PRIVATE_KEY_START.find_all(s)
}

pub(crate) fn private_key_end(s: &str, pos: usize) -> Option<Span> {
    PRIVATE_KEY_END.search(s, pos)
}

const SECRET_QUERY: &[&str] = &[
    "token",
    "code",
    "key",
    "sig",
    "signature",
    "auth",
    "session",
    "access_token",
    "id_token",
    "refresh_token",
    "password",
    "secret",
    "otp",
    "apikey",
    "api_key",
    "client_secret",
    "sessionid",
    "pwd",
    "pass",
    "passwd",
    "pin",
];
const SECRET_QUERY_WORDS: &[&str] = &[
    "signature",
    "token",
    "credential",
    "secret",
    "password",
    "apikey",
    "api_key",
    "hmac",
];
const CURSOR_PARAMS: &[&str] = &[
    "pagetoken",
    "nextpagetoken",
    "continuationtoken",
    "synctoken",
    "cursor",
];

/// formats.py:126: a URL query or fragment parameter that carries a credential (pagination cursors aside).
pub fn secret_param(name: &str) -> bool {
    let k = u::lower(name);
    let flat: String = k.chars().filter(|&c| u::is_alnum(c)).collect();
    if CURSOR_PARAMS.contains(&flat.as_str()) {
        return false;
    }
    SECRET_QUERY.contains(&k.as_str())
        || SECRET_QUERY_WORDS
            .iter()
            .any(|w| k.contains(w) || flat.contains(&w.replace('_', "")))
        || flat.ends_with("sig")
        || flat == "xamzcredential"
        || flat == "xgoogcredential"
}

/// formats.py:136: 13-19 ASCII digits, not all the same, that pass Luhn.
pub fn payment_card(value: &str) -> bool {
    let digits: Vec<u32> = value
        .chars()
        .filter(char::is_ascii_digit)
        .map(|c| c as u32 - 48)
        .collect();
    if !(13..=19).contains(&digits.len()) || digits.iter().all(|&d| d == digits[0]) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                if d * 2 > 9 {
                    d * 2 - 9
                } else {
                    d * 2
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')
}

/// formats.py:144: every URL in `s`, reading the text once: only the first start of a scheme run can match.
pub fn urls(s: &str) -> Vec<Span> {
    let bytes = s.as_bytes();
    let (mut found, mut pos) = (Vec::new(), 0);
    while let Some(i) = s[pos..].find("://") {
        let sep = pos + i;
        let mut run = sep;
        while run > pos && scheme_char(bytes[run - 1]) {
            run -= 1;
        }
        let url = URL_START
            .search_in(s, run, sep)
            .and_then(|(start, _)| URL.match_at(s, start));
        match url {
            Some(m) => {
                found.push(m);
                pos = m.1;
            }
            None => pos = sep + 1,
        }
    }
    found
}

/// formats.py:159: a URL's credentials: its userinfo password (or a token alone as userinfo), and the values of
/// its credential query and fragment parameters.
pub fn url_hits(s: &str, url: Span) -> Vec<Hit> {
    let (start, text) = (url.0, &s[url.0..url.1]);
    let mut out = Vec::new();
    let rest = text.find("://").map_or(0, |i| i + 3);
    let stop = ['/', '?', '#']
        .iter()
        .filter_map(|&c| text[rest..].find(c).map(|i| rest + i))
        .min()
        .unwrap_or(text.len());
    if let Some(at) = text[rest..stop].rfind('@').map(|i| rest + i) {
        if at > rest {
            let info = &text[rest..at];
            match info.find(':') {
                Some(colon) if colon + 1 < info.len() => {
                    out.push(Hit {
                        start: start + rest + colon + 1,
                        end: start + at,
                        kind: "URL password",
                    });
                }
                None if TOKEN_LIKE.fullmatch(info) && info.chars().any(u::is_digit) => {
                    out.push(Hit {
                        start: start + rest,
                        end: start + at,
                        kind: "URL token",
                    });
                }
                _ => {}
            }
        }
    }
    for mark in ['?', '#'] {
        let Some(i) = text[stop..].find(mark).map(|i| stop + i) else {
            continue;
        };
        let end = if mark == '?' {
            text[i + 1..].find('#').map_or(text.len(), |j| i + 1 + j)
        } else {
            text.len()
        };
        let (re, t) = (&PARAM, &text[..end]);
        let mut pos = i + 1;
        while pos <= t.len() {
            let Some(c) = re.search_caps(t, pos) else {
                break;
            };
            let (whole, name, value) = (
                c[0].unwrap_or((pos, pos)),
                c[1].unwrap_or((0, 0)),
                c[2].unwrap_or((0, 0)),
            );
            if value.1 > value.0 && secret_param(&t[name.0..name.1]) {
                out.push(Hit {
                    start: start + value.0,
                    end: start + value.1,
                    kind: "URL credential",
                });
            }
            pos = if whole.1 > whole.0 {
                whole.1
            } else {
                crate::rx::next_char(t, whole.1)
            };
        }
    }
    out
}

/// formats.py:186: where each URL's scheme, host and path lie (a card-like number there is an id, never a card).
pub fn url_paths(s: &str, found: &[Span]) -> Vec<Span> {
    found
        .iter()
        .map(|&(a, b)| (a, s[a..b].find(['?', '#']).map_or(b, |i| a + i)))
        .collect()
}

/// formats.py:195.
pub fn private_key_hits(s: &str) -> Vec<Hit> {
    let (mut out, mut pos, mut endless) = (Vec::new(), 0, s.len() + 1);
    while let Some(begin) = PRIVATE_KEY_START.search(s, pos) {
        let end = if begin.1 < endless {
            PRIVATE_KEY_END.search(s, begin.1)
        } else {
            None
        };
        match end {
            Some(end) => {
                out.push(Hit {
                    start: begin.0,
                    end: end.1,
                    kind: "private key",
                });
                pos = end.1;
            }
            None => {
                endless = endless.min(begin.1);
                let mut stop = KEY_BODY.match_at(s, begin.1).map_or(begin.1, |m| m.1);
                let bytes = s.as_bytes();
                while stop > begin.1 && matches!(bytes[stop - 1], b' ' | b'\t' | b'\r' | b'\n') {
                    stop -= 1;
                }
                out.push(Hit {
                    start: begin.0,
                    end: stop,
                    kind: "private key",
                });
                pos = stop.max(begin.1);
            }
        }
    }
    out
}

/// formats.py:214.
pub fn jwt_hits(s: &str) -> Vec<Hit> {
    let (mut out, mut pos) = (Vec::new(), 0);
    let bytes = s.as_bytes();
    while let Some(start) = JWT_START.search(s, pos) {
        if let Some(token) = JWT.match_at(s, start.0) {
            out.push(Hit {
                start: token.0,
                end: token.1,
                kind: "JSON web token",
            });
            pos = token.1;
        } else {
            // `_TOKEN_RUN.match(s, start).end()`: the run of token characters (formats.py:96), read by hand.
            pos = start.0;
            while pos < bytes.len()
                && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_' || bytes[pos] == b'-')
            {
                pos += 1;
            }
        }
    }
    out
}

/// formats.py:225: a cookie header's value: the rest of its line, or its quoted string.
pub fn cookie_hits(s: &str) -> Vec<Hit> {
    let mut out = Vec::new();
    for m in COOKIE.find_all(s) {
        let quote = u::char_at(s, m.1);
        let quoted = match quote {
            Some('"') => QUOTED_DQ.match_at(s, m.1 + 1),
            Some('\'') => QUOTED_SQ.match_at(s, m.1 + 1),
            _ => None,
        };
        let (a, b) = match quoted {
            Some(close) => (m.1 + 1, close.1 - 1),
            None => {
                // `_LINE_REST` (formats.py:100): up to the line's end.
                let a = m.1;
                let mut b = s[a..].find(['\r', '\n']).map_or(s.len(), |i| a + i);
                while b > a {
                    match u::char_before(s, b) {
                        Some(c) if u::is_space(c) => b -= c.len_utf8(),
                        _ => break,
                    }
                }
                (a, b)
            }
        };
        if b > a {
            out.push(Hit {
                start: a,
                end: b,
                kind: "cookie",
            });
        }
    }
    out
}

/// formats.py:240: a MySQL/MariaDB client's password glued to -p, on the client's own command line.
pub fn mysql_hits(s: &str) -> Vec<Hit> {
    let mut out = Vec::new();
    for m in MYSQL.find_all(s) {
        // `_MYSQL_ARGS` (formats.py:103): the command's own arguments.
        let end = s[m.1..]
            .find(['\r', '\n', '|', ';', '&'])
            .map_or(s.len(), |i| m.1 + i);
        let t = &s[..end];
        let mut pos = m.1;
        while pos <= t.len() {
            let Some(c) = MYSQL_P.search_caps(t, pos) else {
                break;
            };
            let (whole, value) = (c[0].unwrap_or((pos, pos)), c[1].unwrap_or((pos, pos)));
            out.push(Hit {
                start: value.0,
                end: value.1,
                kind: "database password",
            });
            pos = whole.1;
        }
    }
    out
}

/// formats.py:249: payment cards (Luhn) over every window of whole digit groups, and the expiry and CVV written
/// right after one. A number that starts a URL path segment or host label is an id. decision: GATE-212.
pub fn card_hits(s: &str, ids: Option<&[Span]>) -> Vec<Hit> {
    let own;
    let ids = match ids {
        Some(ids) => ids,
        None => {
            own = url_paths(s, &urls(s));
            &own
        }
    };
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    for run in DIGIT_RUN.find_all(s) {
        if !u::longer_than(&s[run.0..run.1], 12) {
            continue;
        }
        // `_GROUP.finditer(s, run.start(), run.end())` (formats.py:120): the runs of ASCII digits.
        let mut groups: Vec<Span> = Vec::new();
        let mut i = run.0;
        while i < run.1 {
            if bytes[i].is_ascii_digit() {
                let a = i;
                while i < run.1 && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                groups.push((a, i));
            } else {
                i += 1;
            }
        }
        if groups.iter().map(|&(a, b)| b - a).sum::<usize>() < 13 {
            continue;
        }
        let mut last: Option<usize> = None;
        for (i, &(a, _)) in groups.iter().enumerate() {
            let (mut plain, mut doubled, mut count) = (0u64, 0u64, 0usize);
            let (first, mut same) = (bytes[a], true);
            for &(ga, gb) in &groups[i..] {
                for &c in &bytes[ga..gb] {
                    let d = u64::from(c - 48);
                    (plain, doubled) = (doubled + d, plain + if d > 4 { d * 2 - 9 } else { d * 2 });
                    count += 1;
                    same = same && c == first;
                }
                if count > 19 {
                    break;
                }
                let id = a > 0
                    && matches!(bytes[a - 1], b'/' | b'.')
                    && ids.iter().any(|&(x, y)| x <= a && gb <= y);
                if count >= 13 && plain % 10 == 0 && !same && !id {
                    out.push(Hit {
                        start: a,
                        end: gb,
                        kind: "payment card",
                    });
                    last = Some(last.unwrap_or(0).max(gb));
                }
            }
        }
        if let Some(last) = last {
            if let Some(after) = AFTER_CARD.match_caps(s, last) {
                if let Some((x, y)) = after[1] {
                    out.push(Hit {
                        start: x,
                        end: y,
                        kind: "card expiry",
                    });
                }
                if let Some((x, y)) = after.get(2).copied().flatten() {
                    if y > x {
                        out.push(Hit {
                            start: x,
                            end: y,
                            kind: "CVV",
                        });
                    }
                }
            }
        }
    }
    out
}

/// `set(pattern.findall(text))` for an alternation of plain needles, longest first (formats.py:77-82): leftmost,
/// never overlapping, as Python's one search reads them.
fn needles_found(text: &str, needles: &[&'static str], held: &mut BTreeSet<&'static str>) {
    let mut i = 0;
    while i < text.len() {
        match needles.iter().find(|n| text[i..].starts_with(**n)) {
            Some(n) => {
                held.insert(n);
                i += n.len();
            }
            None => i = crate::rx::next_char(text, i),
        }
    }
}

/// formats.py:285: every format in `s`, in Python's order. `found_urls`: `urls(s)`, when the caller already read
/// them.
pub fn format_hits(s: &str, found_urls: Option<&[Span]>) -> Vec<Hit> {
    let mut out = Vec::new();
    if !u::longer_than(s, 6) {
        return out; // shorter than any format
    }
    let lower = u::lower(s);
    let mut kinds: Vec<&Entry> = Vec::new();
    if !u::longer_than(s, 2047) {
        let mut found = BTreeSet::new();
        needles_found(s, g::EXACT_NEEDLES, &mut found);
        needles_found(&lower, g::FOLDED_NEEDLES, &mut found);
        let mut held: BTreeSet<&str> = BTreeSet::new();
        for n in found {
            if let Some((_, inside)) = g::IMPLIED.iter().find(|(m, _)| *m == n) {
                held.extend(inside.iter().copied());
            }
        }
        for entry in table() {
            if entry.format.kind != DISCORD && entry.format.needles.iter().any(|n| held.contains(n))
            {
                kinds.push(entry);
            }
        }
    } else {
        for entry in table() {
            let text = if entry.format.folded {
                lower.as_str()
            } else {
                s
            };
            if entry.format.kind != DISCORD && entry.format.needles.iter().any(|n| text.contains(n))
            {
                kinds.push(entry);
            }
        }
    }
    if u::longer_than(s, 58) && s.bytes().filter(|&b| b == b'.').count() >= 2 {
        kinds.extend(table().iter().filter(|e| e.format.kind == DISCORD));
    }
    for entry in kinds {
        for c in entry.rx.caps_all(s) {
            if let Some((a, b)) = c[entry.format.group] {
                out.push(Hit {
                    start: a,
                    end: b,
                    kind: entry.format.kind,
                });
            }
        }
    }
    if s.contains("-----BEGIN") {
        out.extend(private_key_hits(s));
    }
    if s.contains("eyJ") {
        out.extend(jwt_hits(s));
    }
    if lower.contains("cookie") {
        out.extend(cookie_hits(s));
    }
    if lower.contains("mysql") || lower.contains("mariadb") {
        out.extend(mysql_hits(s));
    }
    let own;
    let found = match found_urls {
        Some(found) => found,
        None => {
            own = urls(s);
            &own
        }
    };
    for &url in found {
        out.extend(url_hits(s, url));
    }
    out.extend(card_hits(s, Some(&url_paths(s, found))));
    out
}

/// formats.py:315: whether hit `h` in `s` holds a value: a hit that is only a mask holds none.
pub fn holds_value(s: &str, h: &Hit) -> bool {
    s[h.start..h.end]
        .replace("[redacted]", "")
        .replace("[excluded]", "")
        .chars()
        .any(u::is_alnum)
}

/// formats.py:320: the name of the first format `s` holds, or None (a gate's specific hit).
pub fn first_format(s: &str) -> Option<&'static str> {
    format_hits(s, None)
        .iter()
        .find(|h| holds_value(s, h))
        .map(|h| h.kind)
}

/// formats.py:325: named kinds as a sentence says them, each with its article.
pub fn named_kinds(kinds: &[&str]) -> String {
    let article = |kind: &str| -> String {
        let word = if u::strip(kind).is_empty() {
            kind
        } else {
            u::split_ws(kind).next().unwrap_or(kind)
        };
        let acronym = u::is_upper_str(word) || !word.chars().any(|c| "aeiouAEIOU".contains(c));
        let first: String = word
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        let letters = if acronym { "AEFHILMNORSX" } else { "AEIO" };
        let an = letters.contains(first.as_str()); // Python's `in` on a str: a substring test ("" is in it)
        format!("{}{}", if an { "an " } else { "a " }, kind)
    };
    let named: Vec<String> = kinds.iter().map(|k| article(k)).collect();
    if named.len() < 3 {
        named.join(" and ")
    } else {
        format!(
            "{} and {}",
            named[..named.len() - 1].join(", "),
            named[named.len() - 1]
        )
    }
}

/// formats.py:337: the fact line a model is given beside the owner's words when FERB hid secrets in them; names
/// only, never a value. decision: GATE-502.
pub fn hidden_note(kinds: &[&str], held_in: &str) -> String {
    format!(
        "FERB's note: the owner's {held_in} held {}; FERB hid {}.",
        named_kinds(kinds),
        if kinds.len() == 1 { "it" } else { "them" }
    )
}
