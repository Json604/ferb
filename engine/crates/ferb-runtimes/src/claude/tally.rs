//! What FERB's Claude relay reads from Anthropic's answers (ORCH-719 step 1): each model request's usage, counted as
//! the answer passes through, so a run that never reaches Claude Code's final `result` frame (cancelled, stopped by
//! FERB's guard, failed) still has its tokens recorded, and a run that does is counted in requests, not only in one
//! total (RUN-421); and Anthropic's refusal of a history edited after its thinking blocks were made.
//!
//! Only numbers are kept: no prompt, reply, header or key is read beyond the `usage` objects and an error's type and
//! words. Beside the usage, the reader notes the names of the tools an answer called and its stop reason, for the
//! step record ([`super::steps`]; names only, never a call's arguments).

use serde_json::{json, Map, Value};

use super::steps::{tool_name, Answer};

/// The usage of a run's model requests since it was last taken, summed as Anthropic reports each one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Model requests that reported usage.
    pub requests: i64,
    /// Uncached input (Anthropic's `input_tokens`).
    pub input: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub output: i64,
    /// The model the latest request named.
    pub model: String,
}

impl Tally {
    pub fn is_empty(&self) -> bool {
        self.requests == 0
    }

    /// Adds another tally (a run's usage stashed at its cancel and what came after).
    pub fn add(&mut self, other: &Tally) {
        self.requests += other.requests;
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.output += other.output;
        if !other.model.is_empty() {
            self.model = other.model.clone();
        }
    }

    /// The tally as one Anthropic `usage` object, for [`crate::usage::from_anthropic`].
    pub fn as_anthropic(&self) -> Map<String, Value> {
        let v = json!({
            "input_tokens": self.input,
            "cache_read_input_tokens": self.cache_read,
            "cache_creation_input_tokens": self.cache_write,
            "cache_creation": {"ephemeral_5m_input_tokens": self.cache_write_5m,
                "ephemeral_1h_input_tokens": self.cache_write_1h},
            "output_tokens": self.output,
        });
        v.as_object().cloned().unwrap_or_default()
    }
}

fn int(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_i64).unwrap_or(0)
}

/// One request's usage as Anthropic reports it: in `message_start` (input and cache counts, the first output count)
/// and `message_delta` (the output so far; newer answers repeat the input counts there), or in a whole JSON answer.
#[derive(Clone, Debug, Default)]
struct Request {
    started: bool,
    input: i64,
    cache_read: i64,
    cache_write: i64,
    cache_write_5m: i64,
    cache_write_1h: i64,
    output: i64,
    model: String,
}

impl Request {
    /// A `usage` object: later counts replace earlier ones (each is the request's total so far).
    fn take(&mut self, usage: &Map<String, Value>) {
        let set = |key: &str, slot: &mut i64| {
            if let Some(n) = usage.get(key).and_then(Value::as_i64) {
                *slot = n;
            }
        };
        set("input_tokens", &mut self.input);
        set("cache_read_input_tokens", &mut self.cache_read);
        set("cache_creation_input_tokens", &mut self.cache_write);
        set("output_tokens", &mut self.output);
        if let Some(Value::Object(by_ttl)) = usage.get("cache_creation") {
            self.cache_write_5m = int(by_ttl.get("ephemeral_5m_input_tokens"));
            self.cache_write_1h = int(by_ttl.get("ephemeral_1h_input_tokens"));
        }
    }

    fn into_tally(self) -> Tally {
        Tally {
            requests: 1,
            input: self.input,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            cache_write_5m: self.cache_write_5m,
            cache_write_1h: self.cache_write_1h,
            output: self.output,
            model: self.model,
        }
    }
}

/// Reads one answer's bytes as they stream past (server-sent events or one JSON body) and keeps only its usage.
#[derive(Default)]
pub struct UsageReader {
    line: Vec<u8>,
    whole: Vec<u8>,
    sse: bool,
    too_long: bool,
    request: Request,
    answer: Answer,
}

/// A JSON answer read whole for its usage is at most this large; a bigger one is not read (its usage is lost, never
/// guessed).
const WHOLE_LIMIT: usize = 4 << 20;

impl UsageReader {
    /// `sse`: the answer is `text/event-stream`.
    pub fn new(sse: bool) -> UsageReader {
        UsageReader {
            sse,
            ..UsageReader::default()
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        if self.too_long {
            return;
        }
        if !self.sse {
            if self.whole.len() + chunk.len() <= WHOLE_LIMIT {
                self.whole.extend_from_slice(chunk);
            } else {
                self.whole = Vec::new();
                self.too_long = true;
            }
            return;
        }
        for &b in chunk {
            if b == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.event_line(&line);
            } else if self.line.len() < WHOLE_LIMIT {
                self.line.push(b);
            }
        }
    }

    fn event_line(&mut self, line: &[u8]) {
        let Some(data) = line.strip_prefix(b"data:") else {
            return;
        };
        let Ok(Value::Object(ev)) = serde_json::from_slice::<Value>(data.trim_ascii()) else {
            return;
        };
        match ev.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let message = ev.get("message").and_then(Value::as_object);
                if let Some(Value::Object(u)) = message.and_then(|m| m.get("usage")) {
                    self.request.started = true;
                    self.request.take(u);
                }
                if let Some(m) = message.and_then(|m| m.get("model")).and_then(Value::as_str) {
                    self.request.model = m.to_string();
                }
            }
            Some("message_delta") => {
                if let Some(Value::Object(u)) = ev.get("usage") {
                    self.request.started = true;
                    self.request.take(u);
                }
                if let Some(s) = ev
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.answer.stop = tool_name(s);
                }
            }
            Some("content_block_start") => {
                if let Some(name) = called(ev.get("content_block")) {
                    self.answer.called.push(name);
                }
            }
            _ => {}
        }
    }

    /// The answer's usage, when it reported one.
    pub fn finish(self) -> Option<Tally> {
        self.finish_step().0
    }

    /// The answer's usage, when it reported one, and what it said (None when it was too large to read: never a
    /// guess).
    pub fn finish_step(mut self) -> (Option<Tally>, Option<Answer>) {
        if !self.sse && !self.whole.is_empty() {
            if let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(&self.whole) {
                if let Some(Value::Object(u)) = body.get("usage") {
                    self.request.started = true;
                    self.request.take(u);
                }
                if let Some(m) = body.get("model").and_then(Value::as_str) {
                    self.request.model = m.to_string();
                }
                if let Some(Value::Array(content)) = body.get("content") {
                    self.answer.called = content.iter().filter_map(|b| called(Some(b))).collect();
                }
                if let Some(s) = body.get("stop_reason").and_then(Value::as_str) {
                    self.answer.stop = tool_name(s);
                }
            }
        } else if self.sse && !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.event_line(&line);
        }
        let answer = (!self.too_long).then(|| std::mem::take(&mut self.answer));
        let tally = self
            .request
            .started
            .then(|| std::mem::take(&mut self.request).into_tally());
        (tally, answer)
    }
}

/// The name of the tool a content block calls, when it is a `tool_use` block.
fn called(block: Option<&Value>) -> Option<String> {
    let block = block?;
    (block.get("type").and_then(Value::as_str) == Some("tool_use")).then(|| {
        tool_name(
            block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
    })
}

/// Anthropic's refusal of a request whose history was edited after a thinking block in it was made (the
/// history-editing check of preserved thinking: enforced for accounts made on or after 2026-08-31 on Opus 5.5 and
/// Sonnet 5.5, and for every account on Fable 5.2). Its shape, from the migration guide bundled with the pinned
/// Claude Code 2.1.330 (`shared/model-migration.md`, "Where the check is enforced"): a 400 `invalid_request_error`
/// whose message reads "messages.N.content.M: Invalid `signature` in `thinking` block. The block is bound to a
/// different conversation. ..."; a tampered signature carries the same first clause without the second and is not
/// this refusal.
pub fn history_refused(status: u16, body: &[u8]) -> bool {
    if status != 400 {
        return false;
    }
    let Ok(Value::Object(answer)) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(Value::Object(error)) = answer.get("error") else {
        return false;
    };
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    error.get("type").and_then(Value::as_str) == Some("invalid_request_error")
        && message.contains("`thinking` block")
        && message.contains("bound to a different conversation")
}

/// The owner-facing line FERB journals when Anthropic refused a run's edited history and FERB sent it unedited.
pub const HISTORY_REFUSED_LINE: &str =
    "Claude didn't accept FERB's shortened copy of this task's earlier steps, so \
FERB sent them in full and won't shorten them again in this task. Nothing was lost.";
