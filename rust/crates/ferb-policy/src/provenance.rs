//! policy/provenance.py: matching a consequential call's targets against where they could have come from (S3). Never
//! decides or blocks. A value in the request's words is "owner_words", one in the owner's answers "owner_answer",
//! one in outside content that content's origin label; nothing found is ["unknown"].

use std::collections::HashSet;
use std::sync::OnceLock;

use ferb_core::types::Target;

use crate::confirm::TextForms;
use crate::py;
use crate::rx::once;
use crate::urls;

pub const MIN_LEN: usize = 3;
pub const MIN_PHONE_DIGITS: usize = 6;
/// A phone number as a destination's shape: shorter runs are codes, and dates are left out.
pub const DESTINATION_PHONE_DIGITS: usize = 8;
const LEAD: &str = "\"'([{<";
const TRAIL: &str = "\"')]}>,;:!?.";
const STRUCTURED: &[&str] = &["account", "site", "path"];

/// Where each target value appears, in `target_values` order.
pub type ArgSources = Vec<(String, Vec<String>)>;

/// `arg_sources.get(key, [])`.
pub fn get<'a>(sources: &'a ArgSources, key: &str) -> &'a [String] {
    sources
        .iter()
        .find(|(k, _)| k == key)
        .map_or(&[][..], |(_, v)| v.as_slice())
}

/// `_strip`: a token without its leading and trailing punctuation: (had leading, token, had trailing).
pub(crate) fn strip_token(raw: &str) -> (bool, &str, bool) {
    let (mut tok, mut lead, mut trail) = (raw, false, false);
    while let Some(first) = tok.chars().next() {
        if LEAD.contains(first) {
            tok = &tok[first.len_utf8()..];
            lead = true;
            continue;
        }
        let last = tok.chars().next_back().unwrap_or(first);
        if TRAIL.contains(last) {
            tok = &tok[..tok.len() - last.len_utf8()];
            trail = true;
            continue;
        }
        break;
    }
    (lead, tok, trail)
}

/// `raw_tokens`: whole tokens without framing punctuation, case kept.
pub fn raw_tokens(text: &str) -> Vec<&str> {
    py::split_ws(text)
        .into_iter()
        .map(|raw| strip_token(raw).1)
        .filter(|t| !t.is_empty())
        .collect()
}

/// `embedded`: the email addresses and scheme:// links written in `text`, also glued into markup (GATE-555).
pub fn embedded(text: &str) -> Vec<&str> {
    let re = once!(
        plain,
        r#"[A-Za-z][A-Za-z0-9+.\-]*://[^{S}"'<>()\[\]{}]+|[{W}.+\-]+@[{W}\-]+(?:\.[{W}\-]+)+"#
    );
    re.texts(text)
        .into_iter()
        .map(|m| strip_token(m).1)
        .filter(|t| !t.is_empty())
        .collect()
}

/// `tokens`: the text's whole tokens, lowercased.
pub fn tokens(text: &str) -> Vec<String> {
    raw_tokens(text).into_iter().map(py::lower).collect()
}

fn phone_run_re() -> &'static crate::rx::Compiled {
    once!(plain, r"[+{D}][{D}{S}\-().]{4,}[{D}]")
}

/// `phone_runs`: the digits of each run with a phone number's destination shape, in order.
pub fn phone_runs(text: &str) -> Vec<String> {
    phone_run_re()
        .texts(text)
        .into_iter()
        .filter(|m| phone_shape(m))
        .map(digits)
        .collect()
}

fn phone_value(v: &str) -> bool {
    !v.is_empty()
        && v.chars()
            .all(|c| py::is_decimal(c) || py::is_space(c) || "-+().".contains(c))
}

fn date_shape(v: &str) -> bool {
    let re = once!(
        plain,
        r"\A(?:[{D}]{4}[\-./][{D}]{1,2}[\-./][{D}]{1,2}|[{D}]{1,2}[\-./][{D}]{1,2}[\-./][{D}]{2,4})\z"
    );
    re.full(v)
}

/// `_phone_shape`.
pub(crate) fn phone_shape(value: &str) -> bool {
    let v = py::strip(value);
    phone_value(v) && digits(v).chars().count() >= DESTINATION_PHONE_DIGITS && !date_shape(v)
}

/// `host_of`: the host of a token that is a URL (has ://), as a browser would open it; "" otherwise.
pub fn host_of(token: &str) -> String {
    if !token.contains("://") {
        return String::new();
    }
    urls::parse(token).map(|u| u.host).unwrap_or_default()
}

fn token_amount(token: &str) -> Option<f64> {
    let mut token = token;
    if let Some(first) = token.chars().next() {
        if py::is_sc(first) {
            token = &token[first.len_utf8()..];
        }
    }
    let re = once!(
        plain,
        r"\A(?:([{D}]+(?:\.[{D}]+)?|[{D}]{1,3}(?:,[{D}]{3})+(?:\.[{D}]+)?))\z"
    );
    if !re.full(token) {
        return None;
    }
    py::ascii_digits(&token.replace(',', "")).parse().ok()
}

fn norm_path(token: &str) -> &str {
    if py::char_len(token) > 1 && token.ends_with('/') {
        &token[..token.len() - 1]
    } else {
        token
    }
}

/// `_digits`: the characters `str.isdigit()` takes, in order.
pub(crate) fn digits(text: &str) -> String {
    text.chars().filter(|&c| py::is_digit(c)).collect()
}

/// One word of a text, for telling where a name in it ends (`_named_by_words`): shapes only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub folded: String,
    pub capital: bool,
    pub sentence_start: bool,
    pub tight: bool,
}

/// `str.splitlines()`.
pub(crate) fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = chars.peek() {
                    chars.next();
                    end += 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    for line in splitlines(text) {
        let mut prev = "";
        let mut joined = false;
        for raw in py::split_ws(line) {
            let (lead, tok, trail) = strip_token(raw);
            if let Some(first) = tok.chars().next() {
                out.push(Word {
                    folded: py::casefold(tok),
                    capital: py::is_upper(first),
                    sentence_start: prev.is_empty() || prev.ends_with(['.', '!', '?']),
                    tight: joined && !lead,
                });
            }
            prev = raw;
            joined = !tok.is_empty() && !trail;
        }
    }
    out
}

/// `Text`: one text, folded and tokenized once (`prepare`); every target is matched against it. The value forms
/// the sink reads are made once, when first needed (`confirm`).
#[derive(Debug, Clone)]
pub struct Text {
    pub folded: String,
    pub tokens: HashSet<String>,
    pub paths: HashSet<String>,
    pub hosts: HashSet<String>,
    pub amounts: Vec<f64>,
    pub phones: HashSet<String>,
    pub words: Vec<Word>,
    pub(crate) forms: OnceLock<TextForms>,
}

/// `prepare`.
pub fn prepare(text: &str) -> Text {
    let found = embedded(text);
    let mut toks = tokens(text);
    toks.extend(found.iter().map(|t| py::lower(t)));
    let hosts = raw_tokens(text)
        .into_iter()
        .chain(found.iter().copied())
        .map(host_of)
        .filter(|h| !h.is_empty())
        .collect();
    Text {
        folded: py::casefold(text),
        paths: toks.iter().map(|t| norm_path(t).to_string()).collect(),
        hosts,
        amounts: toks.iter().filter_map(|t| token_amount(t)).collect(),
        phones: phone_run_re().texts(text).into_iter().map(digits).collect(),
        tokens: toks.into_iter().collect(),
        words: words(text),
        forms: OnceLock::new(),
    }
}

/// `_contains`: `needle` in `folded` as a whole phrase (word boundaries by category).
pub(crate) fn contains(folded: &str, needle: &str) -> bool {
    if needle.is_empty() {
        // "".find("") is 0: a whole phrase when the text has no word character at its start
        return folded.chars().next().is_none_or(|c| !py::word_char(c));
    }
    // every occurrence, overlapping ones too (`find(needle, start + 1)`)
    let mut from = 0;
    while let Some(i) = folded[from..].find(needle) {
        let start = from + i;
        let end = start + needle.len();
        let before = folded[..start].chars().next_back();
        let after = folded[end..].chars().next();
        if before.is_none_or(|c| !py::word_char(c)) && after.is_none_or(|c| !py::word_char(c)) {
            return true;
        }
        from = start + folded[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// `target_values`: (key, value) of each target field that is set, in Python's order.
pub fn target_values(target: &Target) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (k, v) in [
        ("account", &target.account),
        ("site", &target.site),
        ("path", &target.path),
        ("contact", &target.contact),
    ] {
        if let Some(v) = v {
            if !v.is_empty() {
                out.push((k.to_string(), v.clone()));
            }
        }
    }
    for (i, r) in target.recipients.iter().enumerate() {
        out.push((format!("recipient:{i}"), r.clone()));
    }
    if let Some(a) = target.amount {
        out.push(("amount".to_string(), ferb_py::json::python_float(a)));
    }
    out
}

fn matches(kind: &str, value: &str, aliases: &[String], text: &Text) -> bool {
    if kind == "amount" {
        let Ok(v) = value.parse::<f64>() else {
            return false;
        };
        return text.amounts.iter().any(|n| (n - v).abs() < 1e-9);
    }
    if kind == "name" && phone_value(value) && digits(value).chars().count() >= MIN_PHONE_DIGITS {
        return text.phones.contains(&digits(value));
    }
    let needles: Vec<String> = std::iter::once(value)
        .chain(aliases.iter().map(String::as_str))
        .filter(|v| py::char_len(py::strip(v)) >= MIN_LEN)
        .map(py::lower)
        .collect();
    let kind = if kind == "name" && !value.chars().any(py::is_space) {
        "token"
    } else {
        kind
    };
    if kind == "name" {
        return needles
            .iter()
            .any(|n| contains(&text.folded, &py::casefold(n)))
            || named_by_words(value, &text.words);
    }
    if needles.iter().any(|n| {
        text.tokens.contains(n) || (kind == "structured" && text.paths.contains(norm_path(n)))
    }) {
        return true;
    }
    kind == "structured" && needles.iter().any(|n| text.hosts.contains(n))
}

fn named_by_words(value: &str, words: &[Word]) -> bool {
    let wanted: HashSet<String> = py::split_ws(value)
        .into_iter()
        .map(|raw| strip_token(raw).1)
        .filter(|t| !t.is_empty())
        .map(py::casefold)
        .collect();
    for (i, word) in words.iter().enumerate() {
        if !wanted.contains(&word.folded) {
            continue;
        }
        let (mut lo, mut hi) = (i, i);
        while lo > 0 && words[lo].tight && joins(&words[lo - 1], &wanted, word.capital) {
            lo -= 1;
        }
        while hi + 1 < words.len()
            && words[hi + 1].tight
            && joins(&words[hi + 1], &wanted, word.capital)
        {
            hi += 1;
        }
        let run = &words[lo..=hi];
        if run.iter().all(|w| wanted.contains(&w.folded))
            && run.iter().any(|w| py::char_len(&w.folded) >= MIN_LEN)
        {
            return true;
        }
    }
    false
}

fn joins(other: &Word, wanted: &HashSet<String>, capital: bool) -> bool {
    wanted.contains(&other.folded) || !capital || (other.capital && !other.sentence_start)
}

/// `destination_shape`: "email", "phone" or "url" when the whole value is one; else None. A shape, never a meaning.
pub fn destination_shape(text: &str) -> Option<&'static str> {
    let value = py::strip(text);
    if value.is_empty() {
        return None;
    }
    if !value.chars().any(py::is_space) {
        let re = once!(plain, r"\A(?:[^{S}@]+@[^{S}@]+\.[^{S}@]+)\z");
        if re.full(value) {
            return Some("email");
        }
        if !host_of(value).is_empty() {
            return Some("url");
        }
    }
    phone_shape(value).then_some("phone")
}

/// `typed_destination`: the recipient a typed text is (an email address or a phone number), without the
/// punctuation around it; a URL addresses no one (GATE-509).
pub fn typed_destination(text: &str) -> Option<String> {
    let value = py::strip(text);
    if phone_shape(value) {
        return Some(value.to_string());
    }
    let stripped = strip_token(value).1;
    (!stripped.is_empty() && matches!(destination_shape(stripped), Some("email") | Some("phone")))
        .then(|| stripped.to_string())
}

/// `sources`: where `value` appears, by `kind` ("name", "structured" or "amount").
pub fn sources(
    value: &str,
    owner_text: &str,
    external_texts: &[(String, String)],
    aliases: &[String],
    kind: &str,
    answer_text: &str,
) -> Vec<String> {
    let external: Vec<(String, Text)> = external_texts
        .iter()
        .map(|(o, t)| (o.clone(), prepare(t)))
        .collect();
    sources_prepared(
        kind,
        value,
        aliases,
        &prepare(owner_text),
        &prepare(answer_text),
        &external,
    )
}

fn sources_prepared(
    kind: &str,
    value: &str,
    aliases: &[String],
    owner: &Text,
    answers: &Text,
    external: &[(String, Text)],
) -> Vec<String> {
    let mut found = Vec::new();
    if matches(kind, value, aliases, owner) {
        found.push("owner_words".to_string());
    }
    if matches(kind, value, aliases, answers) {
        found.push("owner_answer".to_string());
    }
    let mut outside: Vec<String> = external
        .iter()
        .filter(|(_, t)| matches(kind, value, aliases, t))
        .map(|(o, _)| o.clone())
        .collect();
    outside.sort();
    outside.dedup();
    found.extend(outside);
    if found.is_empty() {
        found.push("unknown".to_string());
    }
    found
}

/// `arg_sources`. `home`: the owner's home folder (`Path.home()`), for a path's ~ form.
pub fn arg_sources(
    target: &Target,
    owner_text: &str,
    external_texts: &[(String, String)],
    answer_text: &str,
    home: &str,
) -> ArgSources {
    let external: Vec<(String, Text)> = external_texts
        .iter()
        .map(|(o, t)| (o.clone(), prepare(t)))
        .collect();
    arg_sources_prepared(
        target,
        &prepare(owner_text),
        &external,
        Some(&prepare(answer_text)),
        home,
    )
}

/// `arg_sources_prepared`: over texts already prepared.
pub fn arg_sources_prepared(
    target: &Target,
    owner: &Text,
    external: &[(String, Text)],
    answers: Option<&Text>,
    home: &str,
) -> ArgSources {
    let empty;
    let answers = match answers {
        Some(a) => a,
        None => {
            empty = prepare("");
            &empty
        }
    };
    let mut out = Vec::new();
    for (key, value) in target_values(target) {
        let mut aliases: Vec<String> = Vec::new();
        let kind = if key == "amount" {
            "amount"
        } else if STRUCTURED.contains(&key.as_str())
            || (key.starts_with("recipient:") && value.contains('@'))
        {
            let prefix = format!("{home}/");
            if key == "path" && value.starts_with(&prefix) {
                aliases.push(format!("~{}", &value[home.len()..]));
            }
            "structured"
        } else {
            "name"
        };
        let found = sources_prepared(kind, &value, &aliases, owner, answers, external);
        out.push((key, found));
    }
    out
}
