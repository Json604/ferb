//! The sealed inventory of unread-mail code candidates: a port of `ferb/identity/mailbox.py`. It
//! never chooses an account, a message or a code for an agent.
//!
//! Gmail's Atom feed shows unread inbox entries only. Numeric strings are opaque candidates, not
//! inferred OTPs; the agent judges sender, time and message identity. Subject and body stay
//! sealed (here: [`Secret`]s, as each candidate's code is), since the numeric grammar cannot mask
//! other secret formats.
//!
//! Seams: Python's mailbox is given an async `fetch` (the extension bridge's `_read_feed`); here
//! the caller fetches and passes the feed's text, and a transport failure stays the caller's.
//! Candidate tokens' random bytes (Python's `secrets.token_urlsafe(18)`) come from a
//! [`TokenRandom`]: the system's by default ([`OsRandom`]), injected so a run can be replayed.
//!
//! Differences from Python, each failing closed (the feed counts as unavailable):
//! - dates: `datetime.fromisoformat`'s extended forms only (`YYYY-MM-DD`, then optionally any one
//!   separator and `HH[:MM[:SS[.f…]]]` and an offset); the basic and week forms are refused;
//! - XML: `roxmltree` instead of expat (a feed with a DTD is refused).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

use crate::{py, Secret};

/// `mailbox.py:15`.
pub const FEED_URL: &str = "https://mail.google.com/mail/feed/atom";
/// `mailbox.py:16`.
const NS: &str = "http://purl.org/atom/ns#";
/// `mailbox.py:92-96`.
const COVERAGE: &str = "Unread inbox entries only; numeric candidates of at least four digits, not verified OTPs. \
Subject and body are withheld to keep unsupported secret formats sealed. Alphanumeric and shorter codes are \
unsupported by this inventory. No candidate is selected automatically; insufficient identity evidence requires \
asking the owner or choosing another explicit route.";

/// `mailbox.py:18`.
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+").expect("a valid pattern")
});
/// Python's `\d` and `[\w-]` for one character (`mailbox.py:17`).
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Nd}$").expect("a valid pattern"));
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_-]$").expect("a valid pattern"));

fn matches(class: &Regex, c: char) -> bool {
    class.is_match(c.encode_utf8(&mut [0u8; 4]))
}

/// `mailbox.py:20-26`. `subject` and `summary` may hold codes: they print as the marker.
#[derive(Debug)]
pub struct Message {
    pub id: String,
    pub sender: String,
    pub subject: Secret,
    pub summary: Secret,
    /// `datetime.isoformat()` of the entry's time.
    pub issued: String,
}

/// `mailbox.py:28-31`.
#[derive(Debug)]
pub struct CodeCandidate {
    pub code: Secret,
    pub message_id: String,
}

/// Why a text is no inbox feed: Python's `ValueError` or `ParseError` (`mailbox.py:33-46`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedError {
    /// Not an Atom feed (signed out?), or an entry's time is no date.
    Value,
    /// Not well-formed XML.
    Parse,
}

/// `re.finditer(_CODE, content)`'s group 1, in order: `(?<![\w-])(?:G-)?(\d{4,})(?![\w-])`.
fn codes(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if i > 0 && matches(&WORD, chars[i - 1]) {
            i += 1;
            continue;
        }
        let start = if chars[i..].starts_with(&['G', '-']) {
            i + 2
        } else {
            i
        };
        let mut end = start;
        while end < chars.len() && matches(&DIGIT, chars[end]) {
            end += 1;
        }
        if end - start >= 4 && (end == chars.len() || !matches(&WORD, chars[end])) {
            out.push(chars[start..end].iter().collect());
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn digits(text: &str, len: usize) -> Option<u32> {
    (text.len() == len && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// `HH[:MM[:SS[.f…]]]` -> (hour, minute, second, microsecond), each in `limit`.
fn clock(text: &str, hour_limit: u32) -> Option<(u32, u32, u32, u32)> {
    if !text.is_ascii() {
        return None;
    }
    let (whole, fraction) = match text.find(['.', ',']) {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    };
    let parts: Vec<&str> = whole.split(':').collect();
    if parts.len() > 3 || (fraction.is_some() && parts.len() != 3) {
        return None;
    }
    let part = |i: usize| parts.get(i).map_or(Some(0), |p| digits(p, 2));
    let (hour, minute, second) = (part(0)?, part(1)?, part(2)?);
    let micro = match fraction {
        None => 0,
        Some(f) if !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{:0<6}", &f[..f.len().min(6)]).parse().ok()?
        }
        Some(_) => return None,
    };
    (hour < hour_limit && minute < 60 && second < 60).then_some((hour, minute, second, micro))
}

/// `datetime.fromisoformat(text.replace("Z", "+00:00")).isoformat()` for the extended forms.
fn isoformat(text: &str) -> Option<String> {
    let text = text.replace('Z', "+00:00");
    if !text.is_char_boundary(10.min(text.len())) || text.len() < 10 {
        return None;
    }
    let (date, rest) = text.split_at(10);
    let bytes = date.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (year, month, day) = (
        digits(&date[..4], 4)?,
        digits(&date[5..7], 2)?,
        digits(&date[8..], 2)?,
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => return None,
    };
    if year == 0 || day == 0 || day > days {
        return None;
    }
    if rest.is_empty() {
        return Some(format!("{date}T00:00:00"));
    }
    let mut after = rest.chars();
    after.next()?; // any one separator
    let time = after.as_str();
    let (time, zone) = match time.find(['+', '-']) {
        Some(i) => (&time[..i], Some((&time[i..i + 1], &time[i + 1..]))),
        None => (time, None),
    };
    let (hour, minute, second, micro) = clock(time, 24)?;
    let mut out = format!("{date}T{hour:02}:{minute:02}:{second:02}");
    if micro != 0 {
        out.push_str(&format!(".{micro:06}"));
    }
    if let Some((sign, offset)) = zone {
        let (h, m, s, us) = clock(offset, 24)?;
        let sign = if (h, m, s, us) == (0, 0, 0, 0) {
            "+"
        } else {
            sign
        };
        out.push_str(&format!("{sign}{h:02}:{m:02}"));
        if s != 0 || us != 0 {
            out.push_str(&format!(":{s:02}"));
        }
        if us != 0 {
            out.push_str(&format!(".{us:06}"));
        }
    }
    Some(out)
}

/// ElementTree's `element.text`: the text before the first child element ("" for none).
fn own_text(node: roxmltree::Node<'_, '_>) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.is_element() {
            break;
        }
        if child.is_text() {
            out.push_str(child.text().unwrap_or_default());
        }
    }
    out
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| {
        c.is_element() && c.tag_name().namespace() == Some(NS) && c.tag_name().name() == name
    })
}

fn find_text(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child(node, name).map(own_text).unwrap_or_default()
}

/// `mailbox.py:33-46`: the feed's title and its messages.
pub fn parse_feed(xml_text: &str) -> Result<(String, Vec<Message>), FeedError> {
    let start = xml_text.find("<feed").ok_or(FeedError::Value)?;
    // Python slices to `rfind("</feed>") + 7`, which is character 6 when there is no end tag.
    let end = match xml_text.rfind("</feed>") {
        Some(i) => i + "</feed>".len(),
        None => xml_text
            .char_indices()
            .nth(6)
            .map_or(xml_text.len(), |(i, _)| i),
    };
    let document = roxmltree::Document::parse(xml_text.get(start..end).unwrap_or_default())
        .map_err(|_| FeedError::Parse)?;
    let root = document.root_element();
    let title = find_text(root, "title");
    let mut messages = Vec::new();
    for entry in root.children().filter(|c| {
        c.is_element() && c.tag_name().namespace() == Some(NS) && c.tag_name().name() == "entry"
    }) {
        let issued = Some(find_text(entry, "issued"))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| find_text(entry, "modified"));
        let sender = entry
            .children()
            .filter(|c| {
                c.is_element()
                    && c.tag_name().namespace() == Some(NS)
                    && c.tag_name().name() == "author"
            })
            .find_map(|author| child(author, "email"))
            .map(own_text)
            .unwrap_or_default();
        messages.push(Message {
            id: find_text(entry, "id"),
            sender: sender.to_lowercase(),
            subject: Secret::new(find_text(entry, "title")),
            summary: Secret::new(find_text(entry, "summary")),
            issued: isoformat(&issued).ok_or(FeedError::Value)?,
        });
    }
    Ok((title, messages))
}

/// Random bytes for candidate tokens (Python's `secrets.token_bytes`). `false`: none could be had
/// (the inventory then counts as unavailable).
pub trait TokenRandom: Send + Sync {
    fn fill(&self, buf: &mut [u8]) -> bool;
}

/// The system's random source.
pub struct OsRandom;

impl TokenRandom for OsRandom {
    fn fill(&self, buf: &mut [u8]) -> bool {
        getrandom::fill(buf).is_ok()
    }
}

/// `mailbox.py:50-101`: the candidates of one mailbox (FERB's own address), by opaque token.
pub struct GmailFeedMailbox {
    address: String,
    feed_url: String,
    /// The ids of messages whose code was used; their candidates are no longer listed.
    pub used: HashSet<String>,
    candidates: HashMap<String, CodeCandidate>,
    random: Arc<dyn TokenRandom>,
}

impl GmailFeedMailbox {
    /// Tokens from the system's random source.
    pub fn new(address: &str, feed_url: &str) -> Self {
        Self::with_random(address, feed_url, Arc::new(OsRandom))
    }

    /// Tokens from `random` (a replayed run's source).
    pub fn with_random(address: &str, feed_url: &str, random: Arc<dyn TokenRandom>) -> Self {
        GmailFeedMailbox {
            address: address.to_lowercase(),
            feed_url: feed_url.to_owned(),
            used: HashSet::new(),
            candidates: HashMap::new(),
            random,
        }
    }

    /// `mailbox.py:63-65`: exact address equality, so another inbox whose title merely contains
    /// FERB's address is not accepted.
    fn account_matches(&self, title: &str) -> bool {
        let seen: BTreeSet<String> = EMAIL
            .find_iter(title)
            .map(|m| m.as_str().to_lowercase())
            .collect();
        seen.len() == 1 && seen.contains(&self.address)
    }

    /// `mailbox.py:67-69`, for the feed text the caller fetched from `feed_url`.
    pub fn signed_in(&self, feed_text: &str) -> bool {
        parse_feed(feed_text).is_ok_and(|(title, _)| self.account_matches(&title))
    }

    /// `mailbox.py:71-96`: the inventory a model sees: tokens, senders, times and markers, never
    /// a code, a subject or a body. Replaces the tokens of the inventory before.
    pub fn candidates(&mut self, feed_text: &str) -> Value {
        self.candidates.clear();
        let Ok((title, messages)) = parse_feed(feed_text) else {
            return json!({"available": false, "reason": "feed_unavailable", "candidates": []});
        };
        if !self.account_matches(&title) {
            let observed: BTreeSet<&str> = EMAIL.find_iter(&title).map(|m| m.as_str()).collect();
            return json!({"available": false, "reason": "account_mismatch", "expected_account": self.address,
                          "observed_accounts": observed, "candidates": []});
        }
        let mut out = Vec::new();
        for message in &messages {
            if self.used.contains(&message.id) {
                continue;
            }
            let content = format!("{}\n{}", message.subject.expose(), message.summary.expose());
            let mut found: Vec<String> = Vec::new();
            for code in codes(&content) {
                if !found.contains(&code) {
                    found.push(code);
                }
            }
            for (i, code) in found.into_iter().enumerate() {
                let mut random = [0u8; 18];
                if !self.random.fill(&mut random) {
                    self.candidates.clear();
                    return json!({"available": false, "reason": "feed_unavailable", "candidates": []});
                }
                let token = py::b64encode_urlsafe_nopad(&random);
                let marker = format!("[candidate {}: {} digits]", i + 1, code.chars().count());
                out.push(json!({"candidate": token, "message_id": message.id, "source": self.feed_url,
                                "account": self.address, "sender": message.sender, "issued": message.issued,
                                "marker": marker}));
                self.candidates.insert(
                    token,
                    CodeCandidate {
                        code: Secret::new(code),
                        message_id: message.id.clone(),
                    },
                );
            }
        }
        json!({"available": true, "candidates": out, "coverage": COVERAGE})
    }

    /// `mailbox.py:98-102`: the candidate of one token, once. `None` is Python's `ValueError`
    /// ("OTP candidate is unavailable, consumed or belongs to another inventory").
    pub fn take(&mut self, token: &str) -> Option<CodeCandidate> {
        self.candidates
            .remove(token)
            .filter(|candidate| !self.used.contains(&candidate.message_id))
    }
}
