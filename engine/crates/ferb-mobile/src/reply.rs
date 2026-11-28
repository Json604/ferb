//! Reply to a message (the owner, 2026-11-27, kb APP-1146: "quote any ferb's reply message, either slide or an on hover
//! icon maybe"; "yeah my messages too"), as FERB.app does it (`NotchCore/ReplyQuote.swift`). The owner quotes one
//! message of the chat, his own or FERB's, when he sends a new one: `chat.send`'s `reply_to` names the quoted entry by
//! the engine's id, and the engine keeps `reply_to` {entry_id, kind, who, excerpt} on his message's entry, so every
//! client shows the quote. An id the engine can't quote is dropped there (the message goes without it), never refused.
//!
//! Only a message whose engine entry id the phone holds can be quoted: the fold gives each such bubble its quote
//! (`fold::TurnItem::message_quote` and the others), the app hands one back with the message (`Mobile::send_reply`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How much of a quoted message's words a quote made on the phone keeps (characters; FERB.app's `ReplyQuote.of`, the
/// engine's `REPLY_EXCERPT_CHARS`).
pub const EXCERPT_CHARS: usize = 160;

/// What the owner's message quotes, or what a bubble quotes when he replies to it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReplyQuote {
    /// The engine's entry id of the quoted message.
    pub entry_id: String,
    /// The quoted entry's kind as the engine names it (`message`, `plan`, `answer`, `error`, `notice`, `post`).
    pub kind: String,
    /// The owner's own words (else FERB's).
    pub by_owner: bool,
    /// Its opening words on one line, without Markdown's marks: what the quote shows.
    pub excerpt: String,
}

impl ReplyQuote {
    /// A quote of words the chat shows (FERB.app's `ReplyQuote.of`): their opening on one line. None without an entry
    /// id or words (such a bubble offers no reply).
    pub fn of(entry_id: &str, kind: &str, by_owner: bool, text: &str) -> Option<ReplyQuote> {
        if entry_id.is_empty() || text.trim().is_empty() {
            return None;
        }
        Some(ReplyQuote {
            entry_id: entry_id.to_string(),
            kind: kind.to_string(),
            by_owner,
            excerpt: excerpt(text),
        })
    }

    /// The engine's `reply_to` ({entry_id, kind, who, excerpt}, read tolerantly: only the entry id is required); None
    /// without an entry id.
    pub fn read(v: &Value) -> Option<ReplyQuote> {
        Self::kept(v).map(|q| ReplyQuote {
            excerpt: one_line(&q.excerpt),
            ..q
        })
    }

    /// The same shape as the phone wrote it ([`ReplyQuote::wire`]), its excerpt as kept.
    pub fn kept(v: &Value) -> Option<ReplyQuote> {
        let m = v.as_object()?;
        let id = m.get("entry_id")?.as_str().filter(|id| !id.is_empty())?;
        let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or_default();
        Some(ReplyQuote {
            entry_id: id.to_string(),
            kind: text("kind").to_string(),
            by_owner: m.get("who").and_then(Value::as_str) == Some("owner"),
            excerpt: text("excerpt").to_string(),
        })
    }

    /// The quote as the engine writes it (what the outbox keeps).
    pub fn wire(&self) -> Value {
        json!({"entry_id": self.entry_id, "kind": self.kind,
               "who": if self.by_owner { "owner" } else { "ferb" }, "excerpt": self.excerpt})
    }

    /// A quote the app hands in, as the phone keeps it: its excerpt cut as one made here; None without an entry id.
    pub fn normalized(&self) -> Option<ReplyQuote> {
        (!self.entry_id.trim().is_empty()).then(|| ReplyQuote {
            entry_id: self.entry_id.clone(),
            kind: self.kind.clone(),
            by_owner: self.by_owner,
            excerpt: excerpt(&self.excerpt),
        })
    }
}

/// Words as a quote shows them: on one line, its first [`EXCERPT_CHARS`] characters.
pub fn excerpt(text: &str) -> String {
    one_line(text).chars().take(EXCERPT_CHARS).collect()
}

/// Text as one line (FERB.app's `ReplyQuote.oneLine`): links as their words, a line's heading, list and quote marks
/// out, `**`, `__` and `` ` `` out, each run of white space one space.
pub fn one_line(text: &str) -> String {
    let linked = links_as_words(text);
    let unmarked: Vec<String> = linked.split('\n').map(without_line_mark).collect();
    let mut shown = unmarked.join("\n");
    for mark in ["**", "__", "`"] {
        shown = shown.replace(mark, "");
    }
    shown.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `\[([^\]]*)\]\([^)]*\)` → its words.
fn links_as_words(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == ']') {
                let close = i + 1 + close;
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close + 2..].iter().position(|&c| c == ')') {
                        out.extend(&chars[i + 1..close]);
                        i = close + 2 + end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// A line without its leading `#…# `, `- `, `* `, `+ `, `>` or `1. ` / `1) ` (and the white space before it).
fn without_line_mark(line: &str) -> String {
    let rest = line.trim_start();
    let after_space = |s: &str| -> Option<String> {
        let t = s.trim_start();
        (t.len() < s.len()).then(|| t.to_string())
    };
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        if let Some(t) = after_space(&rest[hashes..]) {
            return t;
        }
    }
    if let Some(c) = rest.chars().next() {
        if matches!(c, '-' | '*' | '+') {
            if let Some(t) = after_space(&rest[1..]) {
                return t;
            }
        }
        if c == '>' {
            return rest[1..].trim_start().to_string();
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && matches!(rest[digits..].chars().next(), Some('.' | ')')) {
        if let Some(t) = after_space(&rest[digits + 1..]) {
            return t;
        }
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: APP-1146 — a quote's words as FERB.app's ReplyQuote.oneLine shows them
    #[test]
    fn a_quote_is_one_line_without_markdown_marks() {
        assert_eq!(
            one_line(
                "## Plan\n- book the **7pm** table at [Nori](https://x.y)\n2. then `call`\n> note"
            ),
            "Plan book the 7pm table at Nori then call note"
        );
        assert_eq!(one_line("#hashtag stays"), "#hashtag stays");
        assert_eq!(one_line("  -not a list"), "-not a list");
        let long = "é".repeat(EXCERPT_CHARS + 5);
        assert_eq!(excerpt(&long).chars().count(), EXCERPT_CHARS);
        assert_eq!(ReplyQuote::of("e1", "answer", false, "  \n "), None);
        assert_eq!(ReplyQuote::of("", "answer", false, "words"), None);
        let q = ReplyQuote::read(
            &serde_json::json!({"entry_id": "e2", "kind": "answer", "who": "owner",
                                                    "excerpt": "Go for **the 7**."}),
        )
        .unwrap();
        assert_eq!(
            q,
            ReplyQuote {
                entry_id: "e2".into(),
                kind: "answer".into(),
                by_owner: true,
                excerpt: "Go for the 7.".into()
            }
        );
        assert_eq!(
            ReplyQuote::read(&serde_json::json!({"kind": "answer"})),
            None
        );
        assert_eq!(ReplyQuote::read(&q.wire()), Some(q));
    }
}
