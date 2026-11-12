//! Port of ferb/sanitize/labels.py: the labelled secret classes: a value a label names as a secret, and a one-time
//! code with its code word near. One label vocabulary (`label_class`) serves dict keys, `key=value` and
//! `"key": value` text, URL parameters and field metadata; prose adds the owner's secret words and code words.
//! Labelled by words, never by digit shape alone. Never a judgment of intent (AGENTS.md rule 4).
//!
//! decision: GATE-211 (the reviewed OTP rules), GATE-572 (the address-code exemption and CODE_REACH 8 kept),
//! GATE-206 (card fields).

use crate::formats::urls;
use crate::generated as g;
use crate::known::REDACTED;
use crate::rx::{Caps, Rx, Span};
use crate::unicode as u;

// ---------- the one vocabulary (labels.py:22-42) ----------

pub const INTERNAL_ID_KEYS: &[&str] = &[
    "task_id",
    "span_id",
    "session_id",
    "run_token_id",
    "request_id",
    "covered",
    "source_refs",
    "procedure_id",
    "approval_id",
    "timer_id",
];
const PROSE_SECRET: &[&str] = &[
    "password",
    "passwords",
    "passwd",
    "pwd",
    "passphrase",
    "passcode",
    "secret",
    "token",
    "otp",
    "totp",
    "pin",
    "cvv",
    "cvc",
    "cvv2",
    "cvc2",
    "csc",
    "apikey",
];
const KEY_SECRET: &[&str] = &[
    "cookie",
    "session",
    "credential",
    "credentials",
    "authorization",
    "auth",
    "bearer",
    "sessionid",
];
const SECRET_COMPOUNDS: &[&str] = &[
    "apikey",
    "accesstoken",
    "refreshtoken",
    "idtoken",
    "clientsecret",
    "privatekey",
    "setcookie",
    "onetimecode",
    "onetimepassword",
    "xapikey",
    "cardnumber",
    "ccnumber",
    "ccnum",
    "cardnum",
    "securitycode",
    "accountkey",
    "sharedaccesskey",
    "secretkey",
    "accesskey",
    "signingkey",
    "encryptionkey",
    "masterkey",
];
const CODE_KEY_WORDS: &[&str] = &["code", "codes", "verification", "2fa"];
const CREDENTIAL_LAST: &[&str] = &["key", "sig", "signature", "dsn", "salt", "hmac"];
const QUALIFIERS: &[&str] = &[
    "id",
    "value",
    "string",
    "str",
    "b64",
    "base64",
    "hex",
    "raw",
    "plain",
    "plaintext",
    "text",
    "data",
    "enc",
    "encrypted",
    "bytes",
];
const PLACE_WORDS: &[&str] = &[
    "pin",
    "zip",
    "postal",
    "post",
    "area",
    "coupon",
    "promo",
    "promotional",
    "discount",
    "voucher",
];

/// What a label names (labels.py:51).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelClass {
    /// Any value.
    Secret,
    /// A code-shaped value.
    Code,
    /// A random-looking value.
    Credential,
}

impl LabelClass {
    /// Python's name for the class.
    pub fn as_str(self) -> &'static str {
        match self {
            LabelClass::Secret => "secret",
            LabelClass::Code => "code",
            LabelClass::Credential => "credential",
        }
    }
}

/// labels.py:45: a label's words ("session_id" -> session, id; "apiKey" -> api, key).
pub fn words(key: &str) -> Vec<String> {
    // `re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", key)` by hand: a match never overlaps the next (it ends on a capital).
    let mut spaced = String::with_capacity(key.len() + 4);
    let mut prev = '\0';
    for c in key.chars() {
        if c.is_ascii_uppercase() && (prev.is_ascii_lowercase() || prev.is_ascii_digit()) {
            spaced.push(' ');
        }
        spaced.push(c);
        prev = c;
    }
    u::lower(&spaced)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// labels.py:51: "secret" (any value), "code" (a code-shaped value), "credential" (a random-looking value) or
/// None. The naming word is the label's last (after qualifiers like "_id", "_value").
pub fn label_class(label: &str) -> Option<LabelClass> {
    if INTERNAL_ID_KEYS.contains(&label) {
        return None;
    }
    let mut ws = words(label);
    if ws.is_empty() {
        return None;
    }
    let is_code = |w: &str| w == "code" || w == "codes";
    if ws
        .windows(2)
        .any(|p| PLACE_WORDS.contains(&p[0].as_str()) && is_code(&p[1]))
        || ws.iter().any(|w| {
            w.strip_suffix("code")
                .is_some_and(|stem| PLACE_WORDS.contains(&stem))
        })
    {
        return None;
    }
    while ws.len() > 1
        && QUALIFIERS.contains(&ws[ws.len() - 1].as_str())
        && !(ws[ws.len() - 1] == "id" && ws[ws.len() - 2] == "key")
    {
        ws.pop(); // "session_id" names the session; "kms_key_id" no key
    }
    let prose = u::strip(label).chars().any(u::is_space);
    let last = ws[ws.len() - 1].as_str();
    if PROSE_SECRET.contains(&last)
        || (!prose && KEY_SECRET.contains(&last))
        || (1..=4)
            .any(|n| SECRET_COMPOUNDS.contains(&ws[ws.len().saturating_sub(n)..].concat().as_str()))
    {
        return Some(LabelClass::Secret);
    }
    if CODE_KEY_WORDS.contains(&last) {
        return Some(LabelClass::Code);
    }
    if !prose && ws.len() >= 2 && CREDENTIAL_LAST.contains(&last) {
        return Some(LabelClass::Credential);
    }
    None
}

/// labels.py:78: a dict key whose value is a secret whatever it is.
pub fn secret_key(key: &str) -> bool {
    label_class(key) == Some(LabelClass::Secret)
}

/// labels.py:86: a value that looks random: 16+ token characters, letters and digits mixed, about 3+ bits per
/// character. The sum is CPython 3.12's `sum()` over floats (Neumaier's compensated sum), in the order `Counter`
/// gives the counts (first seen first), so a value on the 3.0 line falls the same way.
pub fn high_entropy(value: &str) -> bool {
    let token_char = |c: char| c.is_ascii_alphanumeric() || "+/=_.~-".contains(c);
    if !u::longer_than(value, 15)
        || !value.chars().all(token_char)
        || !value.chars().any(u::is_digit)
        || !value.chars().any(u::is_alpha)
    {
        return false;
    }
    let n = value.len() as f64; // all ASCII here
    let mut counts: Vec<(u8, u32)> = Vec::new();
    for b in value.bytes() {
        match counts.iter_mut().find(|(c, _)| *c == b) {
            Some(entry) => entry.1 += 1,
            None => counts.push((b, 1)),
        }
    }
    let (mut total, mut comp) = (0.0f64, 0.0f64);
    for (_, c) in counts {
        let p = f64::from(c) / n;
        let x = p * p.log2();
        let t = total + x;
        if total.abs() >= x.abs() {
            comp += (total - t) + x;
        } else {
            comp += (x - t) + total;
        }
        total = t;
    }
    if comp != 0.0 && comp.is_finite() {
        total += comp;
    }
    -total >= 3.0
}

/// labels.py:95: whether a label of class `cls` hides this value.
pub fn labelled_value(cls: Option<LabelClass>, value: &str) -> bool {
    let v = u::strip(value);
    let Some(cls) = cls else { return false };
    if v.is_empty()
        || matches!(
            u::lower(v).as_str(),
            "null" | "none" | "true" | "false" | "undefined"
        )
    {
        return false;
    }
    match cls {
        LabelClass::Secret => true,
        LabelClass::Code => CODE_VALUE.fullmatch(v),
        LabelClass::Credential => high_entropy(v),
    }
}

// ---------- key-form labels in text (labels.py:107) ----------

pub(crate) static KEY_LABEL: Rx = Rx::new(&g::LABELS_KEY_LABEL);
static VALUE_QUOTES_DQ: Rx = Rx::new(&g::LABELS_VALUE_QUOTES_DQ);
static VALUE_QUOTES_SQ: Rx = Rx::new(&g::LABELS_VALUE_QUOTES_SQ);
static VALUE_QUOTES_BT: Rx = Rx::new(&g::LABELS_VALUE_QUOTES_BT);

/// labels.py:117: where the run of value characters from a position ends, for one text, read once.
pub struct Ends<'a> {
    s: &'a str,
    stop: Option<Vec<usize>>,
    space: Option<Vec<usize>>,
    quote: Option<Vec<usize>>,
}

impl<'a> Ends<'a> {
    pub fn new(s: &'a str) -> Ends<'a> {
        Ends {
            s,
            stop: None,
            space: None,
            quote: None,
        }
    }

    fn next(found: &[usize], pos: usize, len: usize) -> usize {
        let i = found.partition_point(|&x| x < pos);
        found.get(i).copied().unwrap_or(len)
    }

    fn positions(s: &str, pred: impl Fn(char) -> bool) -> Vec<usize> {
        s.char_indices()
            .filter(|&(_, c)| pred(c))
            .map(|(i, _)| i)
            .collect()
    }

    /// Before whitespace, a quote, "<" or ">" (`_STOP`, labels.py:112).
    pub fn stop(&mut self, pos: usize) -> usize {
        let s = self.s;
        let found = self
            .stop
            .get_or_insert_with(|| Self::positions(s, |c| u::is_space(c) || "\"'<>`".contains(c)));
        Self::next(found, pos, s.len())
    }

    /// Before whitespace (`_SPACE`).
    pub fn space(&mut self, pos: usize) -> usize {
        let s = self.s;
        let found = self
            .space
            .get_or_insert_with(|| Self::positions(s, u::is_space));
        Self::next(found, pos, s.len())
    }

    /// Before a double quote (`_DOUBLE_QUOTE`).
    pub fn quote(&mut self, pos: usize) -> usize {
        let s = self.s;
        let found = self
            .quote
            .get_or_insert_with(|| Self::positions(s, |c| c == '"'));
        Self::next(found, pos, s.len())
    }
}

/// `next((i for i in range(a, min(b, a + 4096)) if s[i] in stops), b)`: 4096 is a count of characters.
fn first_of(s: &str, a: usize, b: usize, stops: &str) -> usize {
    if a >= b {
        return b;
    }
    s[a..b]
        .char_indices()
        .take(4096)
        .find(|&(_, c)| stops.contains(c))
        .map_or(b, |(i, _)| a + i)
}

/// labels.py:145: values after identifier-shaped labels the vocabulary names.
pub fn key_label_spans(s: &str, ends: &mut Ends<'_>) -> Vec<Span> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    for m in KEY_LABEL.caps_all(s) {
        let (whole, q, key, op) = (
            m[0].unwrap_or((0, 0)),
            m[1].unwrap_or((0, 0)),
            m[2].unwrap_or((0, 0)),
            m[3].unwrap_or((0, 0)),
        );
        let (key, op, quoted) = (&s[key.0..key.1], &s[op.0..op.1], q.1 > q.0);
        if op == ":"
            && !quoted
            && key.bytes().all(|b| b.is_ascii_alphabetic())
            && !key
                .as_bytes()
                .windows(2)
                .any(|p| p[0].is_ascii_lowercase() && p[1].is_ascii_uppercase())
        {
            continue;
        }
        let cls = label_class(key.trim_start_matches('-'));
        if cls.is_none() {
            continue;
        }
        let start = whole.1;
        let quote = match bytes.get(start) {
            Some(b'"') => Some(&VALUE_QUOTES_DQ),
            Some(b'\'') => Some(&VALUE_QUOTES_SQ),
            Some(b'`') => Some(&VALUE_QUOTES_BT),
            _ => None,
        };
        let (a, b) = if let Some(rx) = quote {
            let Some(close) = rx.match_at(s, start + 1) else {
                continue;
            };
            (start + 1, close.1 - 1)
        } else {
            let (a, mut b) = (start, ends.stop(start));
            let mut stops = String::from(if quoted { ",}]" } else { "" });
            if whole.0 > 0 && matches!(bytes[whole.0 - 1], b'?' | b'&') {
                stops.push('&');
            }
            if !stops.is_empty() {
                b = first_of(s, a, b, &stops);
            }
            while b > a && b".,;:!?)]}".contains(&bytes[b - 1]) {
                b -= 1;
            }
            (a, b)
        };
        if b > a && labelled_value(cls, &s[a..b]) && &s[a..b] != REDACTED {
            out.push((a, b));
        }
    }
    out
}

// ---------- one-time codes (labels.py:176) ----------

pub(crate) static OTP_LABEL: Rx = Rx::new(&g::LABELS_OTP_LABEL);
pub(crate) static CODE_WORD: Rx = Rx::new(&g::LABELS_CODE_WORD);
static TAG_RE: Rx = Rx::new(&g::LABELS_TAG_RE);
pub(crate) static CODE_VALUE: Rx = Rx::new(&g::LABELS_CODE_VALUE);
pub(crate) static CODE_LINK: Rx = Rx::new(&g::LABELS_CODE_LINK);
static PLACES: Rx = Rx::new(&g::LABELS_PLACES);
static ADDRESS_TAIL: Rx = Rx::new(&g::LABELS_ADDRESS_TAIL);
static LABEL_DIRECT: Rx = Rx::new(&g::LABELS_LABEL_DIRECT);
static LABEL_LINK: Rx = Rx::new(&g::LABELS_LABEL_LINK);
static HARD_BREAK: Rx = Rx::new(&g::LABELS_HARD_BREAK);
static SENTENCE_DOT: Rx = Rx::new(&g::LABELS_SENTENCE_DOT);
static ABBREVIATION: Rx = Rx::new(&g::LABELS_ABBREVIATION);
static OTP_TEXT: Rx = Rx::new(&g::LABELS_OTP_TEXT);
/// labels.py:199: a code word reaches a code across at most this many words. decision: GATE-572.
pub const CODE_REACH: usize = 8;
const CODE_STEMS: &[&str] = &["code", "otp", "2fa", "pin", "pass", "verification"];
const GAP_CHARS: usize = 512;

/// `LINE_BREAK.finditer(s)` (labels.py:200: `\r\n|\r|\n`) as (start, end).
pub fn line_breaks(s: &str) -> Vec<Span> {
    let bytes = s.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                out.push((i, i + 2));
                i += 2;
            }
            b'\r' | b'\n' => {
                out.push((i, i + 1));
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// `LINE_BREAK.search(s, pos)`.
pub(crate) fn line_break_from(s: &str, pos: usize) -> Option<Span> {
    let bytes = s.as_bytes();
    let i = pos
        + bytes[pos..]
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')?;
    Some((
        i,
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i + 2
        } else {
            i + 1
        },
    ))
}

fn has_line_break(s: &str) -> bool {
    s.bytes().any(|b| b == b'\r' || b == b'\n')
}

/// labels.py:216: whether `pos` lies in one of the sorted `spans`.
fn inside(spans: &[Span], pos: usize) -> bool {
    let i = spans.partition_point(|&(a, _)| a <= pos);
    i > 0 && pos < spans[i - 1].1
}

fn address_codes(s: &str) -> Vec<Span> {
    PLACES
        .find_all(s)
        .into_iter()
        .filter_map(|chain| {
            ADDRESS_TAIL
                .match_at(s, chain.1)
                .map(|tail| (chain.0, tail.1))
        })
        .collect()
}

/// labels.py:226: the matches outside address codes ("PIN code 560001" is ordinary data).
fn not_address(s: &str, found: Vec<Span>) -> Vec<Span> {
    if found.is_empty() {
        return found;
    }
    let spans = address_codes(s);
    if spans.is_empty() {
        return found;
    }
    found.into_iter().filter(|m| !inside(&spans, m.0)).collect()
}

/// labels.py:234: the code words of `s`, outside address codes and HTML tags.
pub fn code_words(s: &str) -> Vec<Span> {
    let found = not_address(s, CODE_WORD.find_all(s));
    if found.is_empty() || !s.contains('<') {
        return found;
    }
    let tags = TAG_RE.find_all(s);
    if tags.is_empty() {
        return found;
    }
    found.into_iter().filter(|m| !inside(&tags, m.0)).collect()
}

/// labels.py:243: whether a sentence or clause ends between a and b.
fn clause_break(s: &str, a: usize, b: usize) -> bool {
    if HARD_BREAK.search_in(s, a, b).is_some() {
        return true;
    }
    for dot in SENTENCE_DOT.find_all_in(s, a, b) {
        let chunk = &s[u::back(s, dot.0, 24)..dot.0];
        let last_space = chunk.chars().next_back().is_none_or(u::is_space);
        if last_space
            || !u::split_ws(chunk)
                .last()
                .is_some_and(|w| ABBREVIATION.fullmatch(w))
        {
            return true;
        }
    }
    false
}

/// labels.py:258: how many words start between a and b.
fn words_between(s: &str, a: usize, b: usize) -> usize {
    if u::longer_than(&s[a..b], GAP_CHARS) {
        return CODE_REACH + 1;
    }
    let n = u::split_ws(&s[a..b]).count();
    let glued = n > 0
        && a > 0
        && a < s.len()
        && !u::char_before(s, a).is_some_and(u::is_space)
        && !u::char_at(s, a).is_some_and(u::is_space);
    if glued {
        n - 1
    } else {
        n
    }
}

fn label_joined(s: &str, label_end: usize, code: Span) -> bool {
    let gap = &s[label_end..code.0];
    if LABEL_DIRECT.fullmatch(gap) {
        return true;
    }
    !has_line_break(gap)
        && (LABEL_LINK.search(gap, 0).is_some()
            || (u::rstrip(gap).ends_with('(') && s[code.1..].starts_with(')')))
}

/// labels.py:274: codes with their code word on the same line within reach, joined to it.
fn inline_code_spans(s: &str, lower: &str, in_url: &dyn Fn(Span) -> bool) -> Vec<Span> {
    if !CODE_STEMS.iter().any(|stem| lower.contains(stem)) {
        return Vec::new();
    }
    let codes: Vec<Span> = CODE_VALUE
        .find_all(s)
        .into_iter()
        .filter(|&m| !in_url(m))
        .collect();
    let labels: Vec<Span> = if codes.is_empty() {
        Vec::new()
    } else {
        code_words(s).into_iter().filter(|&m| !in_url(m)).collect()
    };
    let mut spans: Vec<Span> = OTP_TEXT
        .caps_all(s)
        .into_iter()
        .filter_map(|c| c[1])
        .filter(|&m| !in_url(m))
        .collect();
    if labels.is_empty() {
        return spans;
    }
    for code in codes {
        let before = labels.partition_point(|&(_, e)| e <= code.0); // bisect_right(ends, code.start())
        let after = labels.partition_point(|&(a, _)| a < code.1); // bisect_left(begins, code.end())
        let by_before = before > 0 && {
            let end = labels[before - 1].1;
            words_between(s, end, code.0) <= CODE_REACH
                && !has_line_break(&s[end..code.0])
                && !clause_break(s, end, code.0)
                && label_joined(s, end, code)
        };
        let by_after = || {
            after < labels.len() && CODE_LINK.match_at(s, code.1).is_some() && {
                let begin = labels[after].0;
                words_between(s, code.1, begin) <= CODE_REACH
                    && !has_line_break(&s[code.1..begin])
                    && !clause_break(s, code.1, begin)
            }
        };
        if by_before || by_after() {
            spans.push(code);
        }
    }
    spans
}

// ---------- a code alone on its line next to its label line (labels.py:301) ----------

static CODE_LEAD: Rx = Rx::new(&g::LABELS_CODE_LEAD);
const MARK_CHARS: &str = ".,;:()[]'\"*_`";

/// labels.py:307.
pub(crate) fn marks_from(s: &str, mut a: usize, b: usize) -> usize {
    while a < b {
        let Some(c) = u::char_at(s, a) else { break };
        if u::is_space(c) || MARK_CHARS.contains(c) {
            a += c.len_utf8();
        } else if c == '<' {
            match TAG_RE.match_in(s, a, b) {
                Some(tag) => a = tag.1,
                None => break,
            }
        } else {
            break;
        }
    }
    a
}

fn marks_to(s: &str, a: usize, mut b: usize) -> usize {
    while b > a {
        let Some(c) = u::char_before(s, b) else { break };
        if u::is_space(c) || MARK_CHARS.contains(c) {
            b -= c.len_utf8();
        } else if c == '>' {
            match s[a..b].rfind('<').map(|i| a + i) {
                Some(lt) if TAG_RE.fullmatch_in(s, lt, b) => b = lt,
                _ => break,
            }
        } else {
            break;
        }
    }
    b
}

/// labels.py:329: where the code lies in a line that is only a code with its marks.
pub fn code_alone_span(line: &str) -> Option<Span> {
    let lead = line.len() - u::lstrip(line).len();
    let a = CODE_LEAD.match_at(line, lead).map_or(lead, |m| m.1);
    let a = marks_from(line, a, line.len());
    let b = marks_to(line, a, line.len());
    (a < b && CODE_VALUE.fullmatch(&line[a..b])).then_some((a, b))
}

/// labels.py:337.
pub fn code_alone(line: &str) -> bool {
    code_alone_span(line).is_some()
}

/// labels.py:341.
pub fn quote_only(line: &str) -> bool {
    u::strip(&line.replace('>', "")).is_empty()
}

/// labels.py:345: a line that labels the code under it.
pub fn label_line(line: &str) -> bool {
    let found = code_words(line);
    let Some(&(_, end)) = found.last() else {
        return false;
    };
    let text = u::rstrip(
        u::rstrip(line).trim_end_matches(['.', ':', '=', '#', '-', '\u{2013}', '\u{2014}']),
    );
    u::split_ws(&line[end..]).count() <= CODE_REACH
        && (end >= text.len() || !clause_break(text, end, text.len()))
}

/// labels.py:355: a line that names the code above it: it starts with a link and holds a code word.
pub fn link_line(line: &str) -> bool {
    let found = code_words(line);
    let Some(&(start, _)) = found.first() else {
        return false;
    };
    CODE_LINK.match_at(line, 0).is_some()
        && !clause_break(line, 0, start)
        && u::split_ws(&line[..start]).count() <= CODE_REACH
}

/// labels.py:363: the lines of `s` that aren't blank, as (start, end).
pub fn content_lines(s: &str) -> Vec<Span> {
    let (mut lines, mut begin) = (Vec::new(), 0);
    for (a, b) in line_breaks(s)
        .into_iter()
        .chain(std::iter::once((s.len(), s.len())))
    {
        if !u::strip(&s[begin..a]).is_empty() {
            lines.push((begin, a));
        }
        begin = b;
    }
    lines
}

/// labels.py:374.
pub fn line_code_spans(s: &str, lines: &[Span]) -> Vec<Span> {
    let mut spans = Vec::new();
    for (i, &(a, b)) in lines.iter().enumerate() {
        let Some(code) = code_alone_span(&s[a..b]) else {
            continue;
        };
        let mut j = i;
        while j > 0 && quote_only(&s[lines[j - 1].0..lines[j - 1].1]) {
            j -= 1;
        }
        let above = if j > 0 {
            &s[lines[j - 1].0..lines[j - 1].1]
        } else {
            ""
        };
        let below = lines.get(i + 1).map_or("", |&(x, y)| &s[x..y]);
        if label_line(above) || link_line(below) {
            spans.push((a + code.0, a + code.1));
        }
    }
    spans
}

// ---------- backup and recovery code lists (labels.py:390) ----------

static CODE_LIST: Rx = Rx::new(&g::LABELS_CODE_LIST);
static LIST_TOKEN: Rx = Rx::new(&g::LABELS_LIST_TOKEN);
static LIST_GAP: Rx = Rx::new(&g::LABELS_LIST_GAP);
static LIST_LEAD: Rx = Rx::new(&g::LABELS_LIST_LEAD);
static LIST_MARKER: Rx = Rx::new(&g::LABELS_LIST_MARKER);

/// labels.py:401: a line of only list codes, after an item marker.
pub fn list_line(line: &str) -> bool {
    let mut body = u::lstrip(line);
    if let Some(marker) = LIST_MARKER.match_at(body, 0) {
        body = &body[marker.1..];
    }
    // `_LIST_SPLIT.split(body)` (labels.py:398): the pieces between runs of whitespace and commas.
    let mut tokens = body
        .split(|c: char| u::is_space(c) || c == ',')
        .filter(|t| !t.is_empty())
        .peekable();
    tokens.peek().is_some() && tokens.all(|t| LIST_TOKEN.fullmatch(t))
}

/// labels.py:410: a line that starts a list: its label, then only codes.
pub fn list_label_line(line: &str) -> bool {
    let found = not_address(line, CODE_LIST.find_all(line));
    let Some(&(_, end)) = found.last() else {
        return false;
    };
    let rest = &line[end..];
    u::strip(&rest.replace(',', " ")).is_empty() || list_line(rest)
}

/// labels.py:417: each code after a backup/recovery/2FA list label; across line breaks only onto lines made only
/// of codes.
pub fn code_list_spans(s: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    for label in not_address(s, CODE_LIST.find_all(s)) {
        let mut pos = label.1;
        loop {
            let gap = LIST_GAP.match_at(s, pos).unwrap_or((pos, pos));
            let mut start = gap.1;
            let breaks = line_breaks(&s[gap.0..gap.1]);
            if let Some(&(_, last_end)) = breaks.last() {
                let line_end = line_break_from(s, start).map_or(s.len(), |m| m.0);
                if !list_line(&s[pos + last_end..line_end]) {
                    break;
                }
                start = LIST_LEAD.match_at(s, start).map_or(start, |m| m.1);
            }
            if !breaks.is_empty() || pos == label.1 {
                if let Some(marker) = LIST_MARKER.match_at(s, start) {
                    start = marker.1;
                }
            }
            let Some(token) = LIST_TOKEN.match_at(s, start) else {
                break;
            };
            spans.push(token);
            pos = token.1;
        }
    }
    spans
}

// ---------- prose secret words (labels.py:442) ----------

pub(crate) static SECRET_WORD: Rx = Rx::new(&g::LABELS_SECRET_WORD);
pub(crate) static SECRET_AFTER: Rx = Rx::new(&g::LABELS_SECRET_AFTER);
static SECRET_DIRECT: Rx = Rx::new(&g::LABELS_SECRET_DIRECT);
static MINE: Rx = Rx::new(&g::LABELS_MINE);
static ENDS_CLAUSE: Rx = Rx::new(&g::LABELS_ENDS_CLAUSE);
const TRAILING: &[u8] = b".,;:!?)]}\"'`";
const SECRET_STEMS: &[&str] = &["pass", "pwd", "pin", "secret", "key", "token"];

/// labels.py:462.
pub fn secret_words(s: &str) -> Vec<Span> {
    not_address(s, SECRET_WORD.find_all(s))
}

/// `SECRET_AFTER.match(s, pos)`: (its end, its `link` group).
pub(crate) fn secret_after(s: &str, pos: usize) -> Option<(usize, Span)> {
    let c: Caps = SECRET_AFTER.match_caps(s, pos)?;
    Some((c[0]?.1, c[2]?))
}

/// labels.py:466: where the value after a secret word lies.
pub fn secret_value(s: &str, word: Span, ends: &mut Ends<'_>) -> Option<Span> {
    let bytes = s.as_bytes();
    let mut end = word.1;
    let mut strong = false;
    let mine = MINE.search_in(s, u::back(s, word.0, 40), word.0).is_some();
    let after = secret_after(s, end);
    let (mut next_line, mut keyed) = (false, false);
    if let Some((after_end, link)) = after {
        let link = &s[link.0..link.1];
        end = after_end;
        strong = link.contains(':') || link.contains('=');
        next_line = has_line_break(link);
        keyed = matches!(
            link.trim_start_matches('\\').chars().next(),
            Some('"') | Some('\'')
        );
    } else {
        end = SECRET_DIRECT.match_at(s, end)?.1;
    }
    if end >= s.len() || u::char_at(s, end).is_some_and(u::is_space) {
        return None;
    }
    // The quote the value opens with: `"`, `'`, or either escaped (`\"`, `\'`).
    let mut quote: &str = match bytes[end] {
        b'"' => "\"",
        b'\'' => "'",
        b'\\' if bytes.get(end + 1) == Some(&b'"') => "\\\"",
        b'\\' if bytes.get(end + 1) == Some(&b'\'') => "\\'",
        _ => "",
    };
    let mut close = 0;
    if !quote.is_empty() {
        // `_QUOTE_CLOSE[quote].search(s, end + len(quote))` (labels.py:455): the quote, or the line's end first.
        let from = end + quote.len();
        let found = s[from..].char_indices().find_map(|(i, c)| {
            if s[from + i..].starts_with(quote) {
                Some(Some(from + i))
            } else if c == '\r' || c == '\n' {
                Some(None)
            } else {
                None
            }
        });
        match found.flatten() {
            Some(at) => close = at,
            None => quote = "",
        }
    }
    let (start, mut stop);
    if !quote.is_empty() {
        (start, stop) = (end + quote.len(), close);
    } else if next_line {
        start = end;
        stop = line_break_from(s, end).map_or(s.len(), |m| m.0);
        while let Some(c) = u::char_before(s, stop) {
            if !u::is_space(c) {
                break;
            }
            stop -= c.len_utf8();
        }
    } else {
        start = end;
        stop = ends.space(end);
        // an unquoted value never runs past a later double quote (JSON text); an unclosed one it starts with stays
        stop = stop.min(ends.quote(start + 1));
        if keyed {
            stop = first_of(s, start, stop, ",}]"); // a JSON value ends at its delimiter
            if matches!(
                u::lower(u::strip(&s[start..stop])).as_str(),
                "true" | "false" | "null"
            ) {
                return None;
            }
        }
        while stop > start && TRAILING.contains(&bytes[stop - 1]) {
            stop -= 1;
        }
        if stop > 0 && bytes[stop - 1] == b'\\' {
            stop -= 1;
        }
    }
    let value = if stop > start { &s[start..stop] } else { "" };
    let length = u::char_len(value);
    if next_line && quote.is_empty() {
        if !value.replace(REDACTED, "").chars().any(u::is_alnum) {
            return None;
        }
    } else if !value.chars().any(u::is_alnum) || value == REDACTED {
        return None; // `_ALNUM` (labels.py:453) is `[^\W_]`: str.isalnum()
    }
    let digit = value.chars().any(u::is_digit);
    let symbol = value.chars().any(|c| !u::is_alnum(c)); // `_NOT_ALNUM`: `[\W_]`
    let ends_clause = ENDS_CLAUSE.match_at(s, stop + quote.len()).is_some();
    let ok = strong
        || (after.is_some()
            && ((mine && (digit || symbol || ends_clause)) || (length >= 4 && (digit || symbol))))
        || (after.is_none() && length >= 4 && digit);
    ok.then_some((start, stop))
}

// ---------- all labelled spans of a text (labels.py:526) ----------

/// labels.py:528: where the labelled classes lie in `s`. A URL's own words never count.
pub fn labelled_spans(s: &str, found_urls: Option<&[Span]>) -> Vec<Span> {
    let own;
    let url_spans: &[Span] = match found_urls {
        Some(found) => found,
        None => {
            own = urls(s);
            &own
        }
    };
    let in_url = |span: Span| -> bool {
        let i = url_spans.partition_point(|&(a, _)| a <= span.0);
        i > 0 && span.1 <= url_spans[i - 1].1
    };
    let mut ends = Ends::new(s);
    let lower = u::lower(s);
    let mut spans: Vec<Span> = if s.contains('=') || s.contains(':') {
        key_label_spans(s, &mut ends)
            .into_iter()
            .filter(|&m| !in_url(m))
            .collect()
    } else {
        Vec::new()
    };
    if SECRET_STEMS.iter().any(|stem| lower.contains(stem)) {
        for word in secret_words(s) {
            if !in_url(word) {
                if let Some(value) = secret_value(s, word, &mut ends) {
                    spans.push(value);
                }
            }
        }
    }
    spans.extend(inline_code_spans(s, &lower, &in_url));
    if (s.contains('\n') || s.contains('\r')) && CODE_STEMS.iter().any(|stem| lower.contains(stem))
    {
        spans.extend(line_code_spans(s, &content_lines(s)));
    }
    if lower.contains("codes") {
        spans.extend(code_list_spans(s));
    }
    spans
}

// ---------- fields, answers (labels.py:549) ----------

static PASSWORD_FIELD: Rx = Rx::new(&g::LABELS_PASSWORD_FIELD);
pub(crate) static PASSWORD_AUTOCOMPLETE: Rx = Rx::new(&g::LABELS_PASSWORD_AUTOCOMPLETE);
pub(crate) static CARD_AUTOCOMPLETE: Rx = Rx::new(&g::LABELS_CARD_AUTOCOMPLETE);
static SIGN_IN_SECRET: Rx = Rx::new(&g::LABELS_SIGN_IN_SECRET);
static DIGITS_ONLY: Rx = Rx::new(&g::LABELS_DIGITS_ONLY);
const TEXT_FIELD_KINDS: &[&str] = &[
    "",
    "text",
    "password",
    "textbox",
    "combobox",
    "axtextfield",
    "axtextarea",
    "axcombobox",
];
pub const FIELD_METADATA: &[&str] = &[
    "autocomplete",
    "name",
    "id",
    "label",
    "placeholder",
    "title",
];
pub const INPUT_TYPES: &[&str] = &[
    "text",
    "password",
    "email",
    "search",
    "tel",
    "url",
    "number",
    "checkbox",
    "radio",
    "button",
    "submit",
    "reset",
    "file",
    "hidden",
    "image",
    "date",
    "datetime-local",
    "month",
    "week",
    "time",
    "color",
    "range",
    "textarea",
];

fn text_field_kind(kind: &str) -> bool {
    TEXT_FIELD_KINDS.contains(&u::casefold(kind).as_str())
}

/// labels.py:564: a text-entry field of this kind that its metadata names a password field. `metadata`: only the
/// strings (Python skips what isn't one).
pub fn password_field(kind: &str, metadata: &[&str]) -> bool {
    text_field_kind(kind)
        && metadata
            .iter()
            .any(|m| PASSWORD_FIELD.search(m, 0).is_some())
}

/// labels.py:570: a code typed or given for a code label.
pub fn labelled_code(label: &str, text: &str) -> bool {
    let value = u::strip(text);
    (OTP_LABEL.search(label, 0).is_some() && DIGITS_ONLY.fullmatch(value))
        || (!code_words(label).is_empty() && CODE_VALUE.fullmatch(value))
}

/// labels.py:578: a text field a secret word (or a secret label of the vocabulary) names.
pub fn labelled_secret(kind: &str, label: &str, text: &str) -> bool {
    !u::strip(text).is_empty()
        && !kind.is_empty()
        && text_field_kind(kind)
        && (!secret_words(label).is_empty() || label_class(label) == Some(LabelClass::Secret))
}

/// labels.py:585: an answer hidden whole.
pub fn hidden_answer(question: &str, text: &str, reply: bool) -> bool {
    let value = u::strip(text);
    if OTP_LABEL.search(question, 0).is_some() && DIGITS_ONLY.fullmatch(value) {
        return true;
    }
    let asked = !not_address(question, SIGN_IN_SECRET.find_all(question)).is_empty();
    asked && (!reply || u::split_ws(value).count() == 1)
}

/// `CODE_VALUE.sub("[redacted]", s)` (redact.py:263).
pub(crate) fn mask_code_values(s: &str) -> String {
    let (mut out, mut kept) = (String::with_capacity(s.len()), 0);
    for (a, b) in CODE_VALUE.find_all(s) {
        out.push_str(&s[kept..a]);
        out.push_str(REDACTED);
        kept = b;
    }
    out.push_str(&s[kept..]);
    out
}
