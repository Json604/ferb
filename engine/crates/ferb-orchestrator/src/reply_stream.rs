//! ferb/orchestrator/reply_stream.py: the planner's provisional public text, streamed only after every field before
//! it validated. A pure reply streams on the reply channel, a task's plan line on the plan channel; a malformed draft
//! is withdrawn. D33 and GATE-581 marks are applied before any prefix that could pass for FERB is shown.
//! `PlainReply` is the front door's plain-text streamer beside it (ORCH-734): the same redaction and line guard, no
//! answer form.
//!
//! decision: ORCH-638, ORCH-630, ORCH-729

use std::sync::Arc;

use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use ferb_sanitize::{RedactionStream, Redactor};

use crate::ports::{Policy, FROM_AI};
use crate::py::{is_alnum, is_space, splitlines_keep};

/// The runtime's mapping of one top-level property to FERB's own answer form (schema_shape.py), when its provider
/// receives the schema reshaped.
pub type FieldMap = Arc<dyn Fn(&str, &Value) -> Value + Send + Sync>;

const PROPERTIES: &[&str] = &[
    "kind",
    "answers_question",
    "task",
    "control",
    "side_entries",
    "text",
];

/// reply_stream.py:17 `_LineGuard`.
struct LineGuard {
    prefix: String,
    started: bool,
    reply: bool,
    needed: usize,
}

impl LineGuard {
    fn new(reply: bool) -> LineGuard {
        LineGuard {
            prefix: String::new(),
            started: false,
            reply,
            needed: if reply { 5 } else { 4 },
        }
    }

    fn mark(&self, policy: &dyn Policy, text: &str) -> String {
        if self.reply {
            if policy.reads_as_ferbs_check(text) {
                format!("{FROM_AI}{text}")
            } else {
                text.to_string()
            }
        } else {
            policy.from_task(text)
        }
    }

    fn feed(&mut self, policy: &dyn Policy, text: &str, last: bool) -> String {
        let mut out = String::new();
        for part in splitlines_keep(text) {
            if self.started {
                out.push_str(part);
            } else {
                self.prefix.push_str(part);
                let letters: String = self
                    .prefix
                    .nfkc()
                    .filter(|c| is_alnum(*c) || (self.reply && *c == ':'))
                    .collect();
                let letters = letters.trim_start_matches(':');
                if letters.chars().count() >= self.needed || part.contains('\n') {
                    let prefix = std::mem::take(&mut self.prefix);
                    out.push_str(&self.mark(policy, &prefix));
                    self.started = true;
                }
            }
            if part.ends_with('\n') {
                self.started = false;
            }
        }
        if last && !self.prefix.is_empty() {
            let prefix = std::mem::take(&mut self.prefix);
            out.push_str(&self.mark(policy, &prefix));
        }
        out
    }
}

/// Not JSON, or JSON in the wrong place: the draft is withdrawn (Python's ValueError / TypeError).
#[derive(Debug)]
struct Bad;

fn hex4(chars: &[char]) -> Option<u32> {
    if chars.len() != 4 || !chars.iter().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(&chars.iter().collect::<String>(), 16).ok()
}

/// reply_stream.py:54 `_character`: one character of a JSON string at byte `pos`: (it, its size in bytes), None
/// when more text is needed.
fn character(raw: &str, pos: usize) -> Result<Option<(String, usize)>, Bad> {
    let rest = &raw[pos..];
    let mut chars = rest.chars();
    let first = chars.next().ok_or(Bad)?;
    if first != '\\' {
        if (first as u32) < 32 {
            return Err(Bad);
        }
        return Ok(Some((first.to_string(), first.len_utf8())));
    }
    let Some(kind) = chars.next() else {
        return Ok(None);
    };
    if kind != 'u' {
        let value = match kind {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return Err(Bad),
        };
        return Ok(Some((value.to_string(), 1 + kind.len_utf8())));
    }
    let digits: Vec<char> = chars.clone().take(4).collect();
    if digits.len() < 4 {
        return Ok(None);
    }
    let high = hex4(&digits).ok_or(Bad)?;
    if (0xD800..=0xDBFF).contains(&high) {
        let low: Vec<char> = chars.skip(4).take(6).collect();
        if low.len() < 6 {
            return Ok(None);
        }
        if low[0] != '\\' || low[1] != 'u' {
            return Err(Bad);
        }
        let low_value = hex4(&low[2..]).ok_or(Bad)?;
        if !(0xDC00..=0xDFFF).contains(&low_value) {
            return Err(Bad);
        }
        let cp = 0x10000 + ((high - 0xD800) << 10) + (low_value - 0xDC00);
        let c = char::from_u32(cp).ok_or(Bad)?;
        return Ok(Some((c.to_string(), 12)));
    }
    if (0xDC00..=0xDFFF).contains(&high) {
        return Err(Bad);
    }
    let c = char::from_u32(high).ok_or(Bad)?;
    Ok(Some((c.to_string(), 6)))
}

/// `json.JSONDecoder().raw_decode(raw, pos)`: the value and where it ends, or None while it can't be read yet.
fn raw_decode(raw: &str, pos: usize) -> Option<(Value, usize)> {
    let mut stream = serde_json::Deserializer::from_str(&raw[pos..]).into_iter::<Value>();
    match stream.next() {
        Some(Ok(v)) => Some((v, pos + stream.byte_offset())),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Opening,
    Key,
    Colon,
    Value,
    Separator,
    Text,
    Closing,
    Complete,
}

/// reply_stream.py:78 `ReplyDraft`.
pub struct ReplyDraft {
    emit: Arc<dyn Fn(Value) + Send + Sync>,
    field: Option<FieldMap>,
    policy: Arc<dyn Policy>,
    redaction: RedactionStream<Arc<Redactor>>,
    redactor: Arc<Redactor>,
    lines: LineGuard,
    raw: String,
    pos: usize,
    state: State,
    metadata: serde_json::Map<String, Value>,
    key: String,
    visible: bool,
    closed: bool,
    channel: &'static str,
}

impl ReplyDraft {
    pub fn new(
        redactor: Arc<Redactor>,
        policy: Arc<dyn Policy>,
        emit: Arc<dyn Fn(Value) + Send + Sync>,
        field: Option<FieldMap>,
    ) -> ReplyDraft {
        ReplyDraft {
            emit,
            field,
            policy,
            redaction: RedactionStream::new(redactor.clone()),
            redactor,
            lines: LineGuard::new(false),
            raw: String::new(),
            pos: 0,
            state: State::Opening,
            metadata: serde_json::Map::new(),
            key: String::new(),
            visible: false,
            closed: false,
            channel: "reply",
        }
    }

    fn emit_text(&mut self, text: &str, last: bool) {
        let safe = self.redaction.feed(text.as_bytes(), last);
        let safe = String::from_utf8_lossy(&safe).into_owned();
        let guarded = self.lines.feed(self.policy.as_ref(), &safe, last);
        if !guarded.is_empty() {
            self.visible = true;
            (self.emit)(json!({"type": format!("{}_delta", self.channel), "text": guarded}));
        }
    }

    fn eligible(&mut self) -> bool {
        let expected: Vec<&str> = PROPERTIES
            .iter()
            .copied()
            .filter(|p| *p != "text")
            .collect();
        if self.metadata.len() != expected.len()
            || !expected.iter().all(|k| self.metadata.contains_key(*k))
        {
            return false;
        }
        let mapped: serde_json::Map<String, Value> = match &self.field {
            Some(f) => self
                .metadata
                .iter()
                .map(|(k, v)| (k.clone(), f(k, v)))
                .collect(),
            None => self.metadata.clone(),
        };
        let metadata = ferb_context::schemas::ungrouped(&Value::Object(mapped));
        if metadata.get("kind").and_then(|v| v.as_str()) == Some("task") {
            let mut with_text = metadata.as_object().cloned().unwrap_or_default();
            with_text.insert("text".into(), json!(""));
            let value = ferb_context::Py::from_value(&Value::Object(with_text));
            if crate::validate::supervisor_outcome_validate(&value).is_err() {
                return false;
            }
            self.channel = "plan";
            return true;
        }
        let kind = metadata.get("kind").and_then(|v| v.as_str());
        matches!(kind, Some("reply") | Some("clarify"))
            && metadata.as_object().is_some_and(|m| {
                m.iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "kind" | "answers_question"))
                    .all(|(_, v)| {
                        matches!(v, Value::Null | Value::Bool(false))
                            || v.as_array().is_some_and(|a| a.is_empty())
                    })
            })
    }

    /// reply_stream.py:125 `feed`.
    pub fn feed(&mut self, fragment: &str) {
        if self.closed {
            return;
        }
        self.raw.push_str(fragment);
        match self.read() {
            Ok(decoded) => {
                if !self.closed {
                    self.emit_text(&decoded, false);
                }
            }
            Err(Bad) => self.abort(),
        }
    }

    fn read(&mut self) -> Result<String, Bad> {
        let mut decoded = String::new();
        while self.pos < self.raw.len() {
            let c = self.raw[self.pos..]
                .chars()
                .next()
                .expect("inside the text");
            if self.state != State::Text && is_space(c) {
                self.pos += c.len_utf8();
                continue;
            }
            match self.state {
                State::Opening => {
                    if c != '{' {
                        return Err(Bad);
                    }
                    self.state = State::Key;
                }
                State::Key => {
                    let Some((key, end)) = raw_decode(&self.raw, self.pos) else {
                        return Ok(decoded);
                    };
                    let Value::String(key) = key else {
                        return Err(Bad);
                    };
                    if self.metadata.contains_key(&key) || !PROPERTIES.contains(&key.as_str()) {
                        return Err(Bad);
                    }
                    self.key = key;
                    self.pos = end;
                    self.state = State::Colon;
                    continue;
                }
                State::Colon => {
                    if c != ':' {
                        return Err(Bad);
                    }
                    self.state = State::Value;
                }
                State::Value => {
                    if self.key == "text" {
                        if c != '"' || !self.eligible() {
                            self.abort();
                            return Ok(String::new());
                        }
                        self.lines = LineGuard::new(self.channel == "reply");
                        self.state = State::Text;
                    } else {
                        let Some((value, end)) = raw_decode(&self.raw, self.pos) else {
                            return Ok(decoded);
                        };
                        self.metadata.insert(self.key.clone(), value);
                        self.pos = end;
                        self.state = State::Separator;
                        continue;
                    }
                }
                State::Separator => {
                    if c != ',' {
                        return Err(Bad);
                    }
                    self.state = State::Key;
                }
                State::Text => {
                    if c == '"' {
                        self.state = State::Closing;
                    } else {
                        let Some((value, size)) = character(&self.raw, self.pos)? else {
                            return Ok(decoded);
                        };
                        decoded.push_str(&value);
                        self.pos += size;
                        continue;
                    }
                }
                State::Closing => {
                    if c != '}' {
                        return Err(Bad);
                    }
                    self.state = State::Complete;
                }
                State::Complete => return Err(Bad),
            }
            self.pos += c.len_utf8();
        }
        Ok(decoded)
    }

    /// reply_stream.py:201 `finish`: the validated final answer; anything else withdraws the draft.
    pub fn finish(&mut self, validated_final: &str) {
        if self.closed {
            return;
        }
        let matches = self.state == State::Complete
            && match (
                serde_json::from_str::<Value>(&self.raw),
                serde_json::from_str::<Value>(validated_final),
            ) {
                (Ok(a), Ok(b)) => ferb_py::value::py_eq(&a, &b),
                _ => false,
            };
        if !matches {
            self.abort();
            return;
        }
        self.emit_text("", true);
        if self.channel == "plan" {
            (self.emit)(json!({"type": "plan_end"}));
        }
        self.closed = true;
    }

    /// reply_stream.py:216 `restart`: the planner looked something up; nothing shown yet starts over.
    pub fn restart(&mut self) {
        if self.visible {
            self.abort();
            return;
        }
        self.raw.clear();
        self.pos = 0;
        self.state = State::Opening;
        self.metadata.clear();
        self.key.clear();
        self.closed = false;
        self.channel = "reply";
        self.redaction = RedactionStream::new(self.redactor.clone());
        self.lines = LineGuard::new(false);
    }

    /// reply_stream.py:226 `abort`.
    pub fn abort(&mut self) {
        if !self.closed && self.visible {
            (self.emit)(json!({"type": format!("{}_abort", self.channel),
                               "reason": "The draft was interrupted; wait for the final reply."}));
        }
        self.closed = true;
    }
}

/// What `abort` and `withdraw` give as the withdrawn draft's reason: `ReplyDraft::abort`'s words, so the apps see
/// the frame they already draw.
const INTERRUPTED: &str = "The draft was interrupted; wait for the final reply.";

type Emit = dyn Fn(Value) + Send + Sync;

/// One channel of a plain answer: the text as the model wrote it (`streamed`), shown through the redaction stream
/// and then the line guard, as `ReplyDraft::emit_text` shows its `text`.
struct Channel {
    name: &'static str,
    reply: bool,
    redaction: RedactionStream<Arc<Redactor>>,
    lines: LineGuard,
    streamed: String,
    visible: bool,
}

impl Channel {
    fn new(name: &'static str, reply: bool, redactor: &Arc<Redactor>) -> Channel {
        Channel {
            name,
            reply,
            redaction: RedactionStream::new(redactor.clone()),
            lines: LineGuard::new(reply),
            streamed: String::new(),
            visible: false,
        }
    }

    fn show(&mut self, policy: &dyn Policy, emit: &Emit, text: &str, last: bool) {
        let safe = self.redaction.feed(text.as_bytes(), last);
        let safe = String::from_utf8_lossy(&safe).into_owned();
        let guarded = self.lines.feed(policy, &safe, last);
        if !guarded.is_empty() {
            self.visible = true;
            emit(json!({"type": format!("{}_delta", self.name), "text": guarded}));
        }
    }

    /// Everything held is shown, then a fresh redaction stream and a guard at the start of a line.
    fn flush(&mut self, policy: &dyn Policy, emit: &Emit, redactor: &Arc<Redactor>) {
        self.show(policy, emit, "", true);
        self.redaction = RedactionStream::new(redactor.clone());
        self.lines = LineGuard::new(self.reply);
    }

    /// A shown draft is withdrawn, and the channel starts empty.
    fn withdraw(&mut self, emit: &Emit, redactor: &Arc<Redactor>) {
        if self.visible {
            emit(json!({"type": format!("{}_abort", self.name), "reason": INTERRUPTED}));
        }
        *self = Channel::new(self.name, self.reply, redactor);
    }
}

/// The front door's plain answer as it streams (kb/plans/front-door.md, "The front door's turn"): no answer form,
/// no JSON, no unescaping. The reply is shown on the reply channel exactly as `ReplyDraft` shows an answer's `text`
/// (the same `RedactionStream`, then `LineGuard` in reply mode: a line in the form of FERB's own checks is marked
/// before any prefix that could pass for one is shown). A hand-off's ack is shown on the plan channel with the guard
/// in task mode (`policy.from_task`), and is withdrawn with `plan_abort` when code refuses the call. Frames are only
/// those `ReplyDraft` sends: `reply_delta`, `reply_abort`, `plan_delta`, `plan_end`, `plan_abort`.
///
/// decision: GATE-581, ORCH-734
pub struct PlainReply {
    emit: Arc<Emit>,
    policy: Arc<dyn Policy>,
    redactor: Arc<Redactor>,
    reply: Channel,
    ack: Channel,
    /// The reply was finished or aborted: nothing more is shown on the reply channel.
    closed: bool,
    /// The run was aborted: nothing more is shown on either channel.
    ended: bool,
}

impl PlainReply {
    pub fn new(redactor: Arc<Redactor>, policy: Arc<dyn Policy>, emit: Arc<Emit>) -> PlainReply {
        PlainReply {
            emit,
            policy,
            reply: Channel::new("reply", true, &redactor),
            ack: Channel::new("plan", false, &redactor),
            redactor,
            closed: false,
            ended: false,
        }
    }

    /// The next piece of the model's plain reply text.
    pub fn feed(&mut self, piece: &str) {
        if self.closed {
            return;
        }
        self.reply.streamed.push_str(piece);
        self.reply
            .show(self.policy.as_ref(), self.emit.as_ref(), piece, false);
    }

    /// A tool call began. Unlike `ReplyDraft::restart`, which withdraws a shown draft (a JSON answer that starts
    /// over is a different answer), what was shown stays: plain text before a call is the front door's own line to
    /// the owner ("let me look that up"), not part of an answer form. What the redaction stream or the guard still
    /// holds is shown now, so the line is whole while the call runs, and the text after the call starts a fresh
    /// line for the guard (a mark is never skipped because the model went on mid-line).
    pub fn restart(&mut self) {
        if self.closed {
            return;
        }
        self.reply
            .flush(self.policy.as_ref(), self.emit.as_ref(), &self.redactor);
    }

    /// The run's final plain answer: every reply piece fed, in order, across calls. Equal to what streamed, the
    /// rest is shown and the reply ends; anything else withdraws the shown draft (`reply_abort`), as
    /// `ReplyDraft::finish` does for an answer that is not the one streamed.
    pub fn finish(&mut self, final_text: &str) {
        if self.closed {
            return;
        }
        if final_text != self.reply.streamed {
            self.abort_reply();
            return;
        }
        self.reply
            .show(self.policy.as_ref(), self.emit.as_ref(), "", true);
        self.closed = true;
    }

    /// The run ended unfinished: a shown reply is withdrawn (`reply_abort`), and so is an ack not yet ended
    /// (`plan_abort`): no promise stays shown for work that did not start.
    pub fn abort(&mut self) {
        self.abort_reply();
        if !self.ended {
            self.ack.withdraw(self.emit.as_ref(), &self.redactor);
        }
        self.ended = true;
    }

    fn abort_reply(&mut self) {
        if !self.closed && self.reply.visible {
            (self.emit)(json!({"type": "reply_abort", "reason": INTERRUPTED}));
        }
        self.closed = true;
    }

    /// The next piece of a hand-off's ack as the runtime streams the call's arguments, shown provisionally on the
    /// plan channel.
    pub fn ack(&mut self, piece: &str) {
        if self.ended {
            return;
        }
        self.ack.streamed.push_str(piece);
        self.ack
            .show(self.policy.as_ref(), self.emit.as_ref(), piece, false);
    }

    /// Code accepted the hand-off and `final_ack` is its ack. Equal to what streamed, the rest is shown and the
    /// plan stream ends (`plan_end`, as `ReplyDraft::finish` ends a plan line); anything else withdraws it. A later
    /// call's ack streams afresh.
    pub fn ack_end(&mut self, final_ack: &str) {
        if self.ended {
            return;
        }
        if final_ack != self.ack.streamed {
            self.withdraw();
            return;
        }
        self.ack
            .show(self.policy.as_ref(), self.emit.as_ref(), "", true);
        if self.ack.visible {
            (self.emit)(json!({"type": "plan_end"}));
        }
        self.ack = Channel::new("plan", false, &self.redactor);
    }

    /// Code refused the hand-off: a shown ack is withdrawn (`plan_abort`); a repaired call's ack streams afresh.
    pub fn withdraw(&mut self) {
        if self.ended {
            return;
        }
        self.ack.withdraw(self.emit.as_ref(), &self.redactor);
    }
}
