//! Turning imported conversations into memory facts (`import_extract.py`, MEM-400..413): parts and chunks with a
//! stable reference per message, the owner index that finds the owner's messages a saved-memory claim's words
//! appear in, and code's checks of the model's recall and verify answers (a candidate or verdict is kept only
//! when its quote is whole words of the owner message it cites). Saving the verified facts (`save`, `said_at`)
//! belongs to the items part and is not ported here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::import_sources::{source_name, Conversation, Message};
use crate::pymath;
use crate::pystr;
use crate::pyval::{self, Crash};
use crate::terms::{self, hex, Fact};
use crate::times::{self, Date, DateTime, LocalZone};
use crate::Error;

/// One recall call's conversations (~6k tokens).
pub const CHUNK_CHARS: usize = 24_000;
/// The owner's messages shown with a claim.
pub const EVIDENCE_PER_CLAIM: usize = 3;
/// Held facts looked up per candidate.
pub const SIMILAR_PER_CANDIDATE: usize = 3;
/// Held facts shown to one verification call.
pub const SIMILAR_MOST: usize = 30;
pub const REVIEW_DAYS_MOST: i64 = 3650;

/// `RECALL_SCHEMA`.
pub fn recall_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["candidates"],
        "properties": {"candidates": {"type": "array", "items": {
            "type": "object", "additionalProperties": false,
            "required": ["kind", "subject", "key", "content", "message", "quote"],
            "properties": {"kind": {"type": "string", "enum": terms::KINDS}, "subject": {"type": "string"},
                           "key": {"type": "string"}, "content": {"type": "string"}, "message": {"type": "string"},
                           "quote": {"type": "string"}}}}}})
}

/// `VERIFY_SCHEMA`.
pub fn verify_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["verdicts"],
        "properties": {"verdicts": {"type": "array", "items": {
            "type": "object", "additionalProperties": false,
            "required": ["id", "supported", "kind", "subject", "key", "content", "message", "quote", "same_as",
                         "ended", "until", "review_in_days"],
            "properties": {"id": {"type": "string"}, "supported": {"type": "boolean"},
                           "kind": {"type": "string", "enum": terms::KINDS}, "subject": {"type": "string"},
                           "key": {"type": "string"}, "content": {"type": "string"}, "message": {"type": "string"},
                           "quote": {"type": "string"}, "same_as": {"type": "string"}, "ended": {"type": "boolean"},
                           "until": {"type": "string"}, "review_in_days": {"type": "integer"}}}}}})
}

/// `digest(*parts)`: SHA-256 of the parts joined by U+001F.
pub fn digest(parts: &[&str]) -> String {
    hex(&Sha256::digest(parts.join("\x1f").as_bytes()))
}

/// One conversation, or a slice of a long one, as the model reads it.
#[derive(Clone, Debug)]
pub struct Part {
    /// Its hash: what progress keeps.
    pub key: String,
    pub conversation: Arc<Conversation>,
    pub lines: Vec<String>,
    /// reference -> message, in order.
    pub refs: Vec<(String, Message)>,
}

impl Part {
    pub fn chars(&self) -> usize {
        self.lines.iter().map(|l| pystr::char_len(l) + 1).sum()
    }
}

/// Parts packed for one recall call.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub source: String,
    pub parts: Vec<Part>,
}

impl Chunk {
    pub fn text(&self) -> String {
        let body: Vec<String> = self.parts.iter().map(|p| p.lines.join("\n")).collect();
        format!(
            "Source: {}\n\n{}",
            source_name(&self.source).unwrap_or(&self.source),
            body.join("\n\n")
        )
    }

    /// reference -> (message, conversation), in order (a later part's reference replaces an earlier one).
    pub fn refs(&self) -> Vec<(String, Message, Arc<Conversation>)> {
        let mut out: Vec<(String, Message, Arc<Conversation>)> = Vec::new();
        for p in &self.parts {
            for (r, m) in &p.refs {
                match out.iter_mut().find(|(k, _, _)| k == r) {
                    Some(slot) => {
                        slot.1 = m.clone();
                        slot.2 = p.conversation.clone();
                    }
                    None => out.push((r.clone(), m.clone(), p.conversation.clone())),
                }
            }
        }
        out
    }
}

fn tag(conv: &Conversation) -> String {
    format!("c{}", &digest(&[&conv.source, &conv.id])[..6])
}

fn when(m: &Message, zone: &dyn LocalZone) -> String {
    match &m.at {
        Some(at) => {
            times::astimezone(at, zone).map_or_else(|| "undated".into(), |d| times::ymd(&d))
        }
        None => "undated".into(),
    }
}

fn line(r: &str, m: &Message, claims: bool, zone: &dyn LocalZone) -> String {
    let who = if claims {
        "saved memory (the assistant's claim)"
    } else {
        m.role.as_str()
    };
    format!("[{r} · {who} · {}] {}", when(m, zone), m.text)
}

/// `parts`: every conversation as parts within `limit` characters, cut on message boundaries.
pub fn parts(conversations: &[Conversation], limit: usize, zone: &dyn LocalZone) -> Vec<Part> {
    let mut out = Vec::new();
    for conv in conversations {
        let conv = Arc::new(conv.clone());
        let t = tag(&conv);
        let title = if conv.title.is_empty() {
            "untitled"
        } else {
            &conv.title
        };
        let head = format!(
            "{} {t} — “{title}”",
            if conv.claims {
                "Saved memory"
            } else {
                "Conversation"
            }
        );
        let (mut lines, mut refs, mut n) = (vec![head.clone()], Vec::new(), 0usize);
        let key = |n: usize, lines: &[String]| {
            let mut p: Vec<&str> = vec![&conv.source, &conv.id];
            let ns = n.to_string();
            p.push(&ns);
            p.extend(lines.iter().map(String::as_str));
            digest(&p)
        };
        for (i, m) in conv.messages.iter().enumerate() {
            let r = format!("{t}.{}", i + 1);
            let l = line(&r, m, conv.claims, zone);
            let used: usize = lines.iter().map(|x| pystr::char_len(x) + 1).sum();
            if !refs.is_empty() && used + pystr::char_len(&l) > limit {
                out.push(Part {
                    key: key(n, &lines),
                    conversation: conv.clone(),
                    lines: std::mem::take(&mut lines),
                    refs: std::mem::take(&mut refs),
                });
                n += 1;
                lines = vec![format!("{head} (part {})", n + 1)];
            }
            lines.push(l);
            refs.push((r, m.clone()));
        }
        out.push(Part {
            key: key(n, &lines),
            conversation: conv.clone(),
            lines,
            refs,
        });
    }
    out
}

/// `chunks`: parts packed in order into chunks of about `limit` characters (a part over it stays whole, alone).
pub fn chunks(source: &str, todo: &[Part], limit: usize) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = Vec::new();
    for part in todo {
        if let Some(last) = out.last_mut() {
            if last.parts.iter().map(Part::chars).sum::<usize>() + part.chars() <= limit {
                last.parts.push(part.clone());
                continue;
            }
        }
        out.push(Chunk {
            source: source.to_owned(),
            parts: vec![part.clone()],
        });
    }
    out
}

/// The owner's messages of one source, for finding the ones a claim's words appear in.
pub struct OwnerIndex {
    pub messages: Vec<(String, Message, Vec<String>)>,
    pub weight: HashMap<String, f64>,
}

impl OwnerIndex {
    pub fn new(conversations: &[Conversation]) -> OwnerIndex {
        let mut messages = Vec::new();
        let mut df: HashMap<String, usize> = HashMap::new();
        for conv in conversations.iter().filter(|c| !c.claims) {
            let t = tag(conv);
            for (i, m) in conv.messages.iter().enumerate() {
                if m.role == "owner" {
                    let words = terms::query_terms(&m.text);
                    for w in &words {
                        *df.entry(w.clone()).or_default() += 1;
                    }
                    messages.push((format!("{t}.{}", i + 1), m.clone(), words));
                }
            }
        }
        let n = messages.len().max(1);
        let weight = df
            .into_iter()
            .filter(|(_, c)| n < 2 || (*c as f64) <= f64::max(2.0, n as f64 / 2.0))
            .map(|(t, c)| (t, ((n + 1) as f64 / c as f64).ln()))
            .collect();
        OwnerIndex { messages, weight }
    }

    /// `find`: the owner messages sharing the most weighted words with `text`, best first, at most `most`.
    pub fn find(&self, text: &str, most: usize) -> Vec<(String, Message)> {
        let words: HashSet<String> = terms::query_terms(text).into_iter().collect();
        let mut scored: Vec<(f64, &String, &Message)> = self
            .messages
            .iter()
            .filter(|(_, _, mt)| mt.iter().any(|t| words.contains(t)))
            .map(|(r, m, mt)| {
                let shared = mt
                    .iter()
                    .filter(|t| words.contains(*t))
                    .map(|t| self.weight.get(t).copied().unwrap_or(0.0));
                (pymath::sum(shared), r, m)
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(most)
            .filter(|(s, _, _)| *s > 0.0)
            .map(|(_, r, m)| (r.clone(), m.clone()))
            .collect()
    }
}

/// A recalled candidate fact.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    /// The reference it rests on.
    pub message: String,
    pub quote: String,
    /// Proposed from an assistant's saved memory.
    pub claim: bool,
    /// (reference, message, how shown).
    pub evidence: Vec<(String, Message, String)>,
}

/// A fact the verifier supported, resting on an owner message.
#[derive(Clone, Debug, PartialEq)]
pub struct Verified {
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    pub said_at: DateTime,
    pub source: String,
    pub same_as: String,
    pub ended: bool,
    pub until: Option<Date>,
    pub review_in_days: i64,
    pub id: String,
}

fn strip_brackets(s: &str) -> &str {
    s.trim_matches(['[', ']', ' '])
}

fn get_str(v: &Value, key: &str, default: &str) -> Result<String, Error> {
    Ok(pyval::get(v, key)?.map_or_else(|| default.to_owned(), pyval::to_str))
}

/// `check_recall`: recall's candidates that rest on the owner's own words (or a saved memory's claim), with their
/// evidence; and why the others were dropped. What the model wrote is redacted again.
pub fn check_recall(
    answer: &Value,
    chunk: &Chunk,
    index: &OwnerIndex,
    redact: &dyn Fn(&str) -> String,
) -> Result<(Vec<Candidate>, Vec<String>), Error> {
    let refs = chunk.refs();
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    for (n, c) in pyval::iter_or_empty(pyval::get(answer, "candidates")?)?
        .iter()
        .enumerate()
    {
        let r = strip_brackets(&get_str(c, "message", "")?).to_owned();
        let Some((_, msg, conv)) = refs.iter().find(|(k, _, _)| *k == r) else {
            dropped.push("cites no message of this chunk".to_owned());
            continue;
        };
        if msg.role != "owner" && !conv.claims {
            dropped.push("rests on an assistant's or tool's words, not the owner's".to_owned());
            continue;
        }
        let quote = redact(&get_str(c, "quote", "")?);
        if !terms::quoted(&msg.text, &quote) {
            dropped.push("the quote isn't in that message".to_owned());
            continue;
        }
        let kind = pyval::to_str(pyval::get(c, "kind")?.ok_or(Crash("KeyError"))?);
        let field = |k: &str, default: &str| -> Result<String, Error> {
            Ok(match pyval::get(c, k)? {
                Some(v) if pyval::truthy(v) => pyval::to_str(v),
                _ => default.to_owned(),
            })
        };
        let mut cand = Candidate {
            id: format!("v{}", n + 1),
            kind,
            subject: redact(&field("subject", "owner")?),
            key: redact(&field("key", "")?),
            content: redact(&pystr::squeeze(&field("content", "")?)),
            message: r.clone(),
            quote,
            claim: conv.claims,
            evidence: Vec::new(),
        };
        if cand.content.is_empty() || cand.key.is_empty() {
            dropped.push("empty".to_owned());
            continue;
        }
        if conv.claims {
            cand.evidence.push((
                r.clone(),
                msg.clone(),
                "saved memory (the assistant's claim)".into(),
            ));
            for (er, em) in index.find(
                &format!("{} {}", cand.content, cand.quote),
                EVIDENCE_PER_CLAIM,
            ) {
                cand.evidence.push((er, em, "owner".into()));
            }
        } else {
            let ordered = &conv.messages;
            let at = ordered
                .iter()
                .position(|m| m == msg)
                .ok_or(Crash("ValueError"))?;
            let t = r.rsplit_once('.').map_or(r.as_str(), |(a, _)| a);
            let from = at.saturating_sub(1);
            for (i, m) in ordered.iter().enumerate().skip(from).take(at + 2 - from) {
                cand.evidence
                    .push((format!("{t}.{}", i + 1), m.clone(), m.role.clone()));
            }
        }
        kept.push(cand);
    }
    Ok((kept, dropped))
}

/// `verify_context`: the candidates with their evidence and the held facts that look alike.
pub fn verify_context(
    source: &str,
    candidates: &[Candidate],
    held: &[Fact],
    zone: &dyn LocalZone,
) -> String {
    let mut lines = vec![
        format!("Source: {}", source_name(source).unwrap_or(source)),
        String::new(),
    ];
    for c in candidates {
        lines.push(format!(
            "Candidate {}: kind={} subject={} key={}",
            c.id, c.kind, c.subject, c.key
        ));
        lines.push(format!("  says: {}", c.content));
        lines.push(format!("  quoted: “{}” (from {})", c.quote, c.message));
        lines.push("  evidence:".into());
        for (r, m, who) in &c.evidence {
            lines.push(format!("    [{r} · {who} · {}] {}", when(m, zone), m.text));
        }
        lines.push(String::new());
    }
    lines.push("What FERB already remembers that looks alike (data):".into());
    for it in held {
        let day =
            times::astimezone(&it.valid_from, zone).map_or_else(String::new, |d| times::ymd(&d));
        lines.push(format!(
            "  [{}] {} / {} / {}: {} ({}, as of {day})",
            it.id,
            it.kind,
            it.subject,
            it.key,
            it.content,
            terms::label(&it.origin)
        ));
    }
    if held.is_empty() {
        lines.push("  (nothing)".into());
    }
    lines.join("\n")
}

/// `check_verify`: the verifier's supported facts that rest on an owner message shown as evidence, whose quote
/// is whole words of that message; and why the others were dropped.
pub fn check_verify(
    answer: &Value,
    candidates: &[Candidate],
    held_ids: &HashSet<String>,
    source: &str,
) -> Result<(Vec<Verified>, Vec<String>), Error> {
    let by_id: HashMap<&str, &Candidate> = candidates.iter().map(|c| (c.id.as_str(), c)).collect();
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    let mut seen: HashSet<String> = HashSet::new();
    for v in pyval::iter_or_empty(pyval::get(answer, "verdicts")?)? {
        let id = pyval::get(&v, "id")?.map_or_else(|| "None".to_owned(), pyval::to_str);
        let Some(c) = by_id.get(id.as_str()).filter(|c| !seen.contains(&c.id)) else {
            dropped.push("names no candidate".to_owned());
            continue;
        };
        seen.insert(c.id.clone());
        if !pyval::get(&v, "supported")?.is_some_and(pyval::truthy) {
            dropped.push("not supported by the owner's words".to_owned());
            continue;
        }
        let mut owner: Vec<(&str, &Message)> = Vec::new();
        for (r, m, who) in &c.evidence {
            if who == "owner" {
                match owner.iter_mut().find(|(k, _)| *k == r) {
                    Some(slot) => slot.1 = m,
                    None => owner.push((r, m)),
                }
            }
        }
        let cited = get_str(&v, "message", "")?;
        let cited = strip_brackets(&cited);
        let msg = owner.iter().find(|(k, _)| *k == cited).map(|(_, m)| *m);
        let Some((msg, said_at)) = msg.and_then(|m| m.at.map(|at| (m, at))) else {
            dropped.push("rests on no owner message shown".to_owned());
            continue;
        };
        if !terms::quoted(&msg.text, &get_str(&v, "quote", "")?) {
            dropped.push("the quote isn't in that owner message".to_owned());
            continue;
        }
        let field = |k: &str| -> Result<Option<String>, Error> {
            Ok(match pyval::get(&v, k)? {
                Some(x) if pyval::truthy(x) => Some(pyval::to_str(x)),
                _ => None,
            })
        };
        let content = pystr::squeeze(&field("content")?.unwrap_or_default());
        if content.is_empty() {
            dropped.push("empty".to_owned());
            continue;
        }
        let until = match field("until")? {
            Some(u) if !pystr::strip(&u).is_empty() => {
                Date::from_isoformat(pystr::prefix(pystr::strip(&u), 10))
            }
            _ => None,
        };
        let same = field("same_as")?.unwrap_or_default();
        let same = strip_brackets(&same).to_owned();
        let review = match pyval::get(&v, "review_in_days")? {
            Some(x) if pyval::truthy(x) => pyval::int(x)?,
            _ => 0,
        };
        kept.push(Verified {
            kind: field("kind")?.unwrap_or_else(|| c.kind.clone()),
            subject: field("subject")?.unwrap_or_else(|| c.subject.clone()),
            key: field("key")?.unwrap_or_else(|| c.key.clone()),
            content,
            said_at,
            source: source.to_owned(),
            same_as: if held_ids.contains(&same) || by_id.contains_key(same.as_str()) {
                same
            } else {
                String::new()
            },
            ended: pyval::get(&v, "ended")?.is_some_and(pyval::truthy),
            until,
            review_in_days: review.clamp(0, REVIEW_DAYS_MOST),
            id: c.id.clone(),
        });
    }
    Ok((kept, dropped))
}

/// `as_json`: a model's answer as an object (text parsed first).
pub fn as_json(value: &Value) -> Result<Map<String, Value>, Error> {
    let parsed = match value {
        Value::String(s) => serde_json::from_str(s).map_err(|_| Crash("JSONDecodeError"))?,
        v => v.clone(),
    };
    match parsed {
        Value::Object(m) => Ok(m),
        _ => Err(Error::Refused("not an object".into())),
    }
}

/// `similar_held`: the current held facts that look like these candidates (the fused search's answer per
/// candidate, `similar`), at most [`SIMILAR_MOST`].
pub fn similar_held(
    lookup: &dyn Fn(&str) -> Result<Option<Fact>, Error>,
    candidates: &[Candidate],
    similar: &mut dyn FnMut(&str) -> Vec<String>,
    now: &DateTime,
) -> Result<Vec<Fact>, Error> {
    let mut ids: Vec<String> = Vec::new();
    for c in candidates {
        let found = similar(&format!("{} {}", c.key.replace('_', " "), c.content));
        for i in found.into_iter().take(SIMILAR_PER_CANDIDATE) {
            if !ids.contains(&i) {
                ids.push(i);
            }
        }
    }
    let mut out = Vec::new();
    for id in ids.iter().take(SIMILAR_MOST * 2) {
        let Some(it) = lookup(id)? else { continue };
        if it.status == "active" && terms::current(&it, now)? && !it.set_aside {
            out.push(it);
        }
    }
    out.truncate(SIMILAR_MOST);
    Ok(out)
}
