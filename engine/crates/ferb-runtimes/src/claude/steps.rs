//! What FERB's Claude relay keeps about each model step besides its usage (T-1338, the front door's step 1): the
//! names of the tools the answer called, the name and error flag of each tool result the request carried back, the
//! answer's stop reason, its HTTP status when it was not a success, and two times on the relay's clock. So one
//! `model.usage` row says which tool was called, whether its result was an error and whether it was called again.
//!
//! Only names, flags and numbers are kept (rule 7): never a tool's arguments, a result's text or its length, a prompt,
//! a header or a key. A name is kept only in the characters Anthropic allows a tool's name (`[A-Za-z0-9_-]`, at most
//! 128); anything else is kept as "?".

use std::collections::{HashMap, VecDeque};
use std::fmt;

use serde::de::{self, Deserializer, IgnoredAny, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// The steps one record keeps: the latest this many; the earlier ones are only counted (`steps_dropped`). An agent's
/// bound on a trace record's size (about 150 bytes a step), not a limit on anything FERB does.
pub const KEPT: usize = 64;

/// The longest tool name kept as it is (Anthropic's own bound on a tool's name).
const NAME_MOST: usize = 128;

/// A tool's name as it may be kept: its own when it is a name Anthropic accepts, else "?".
pub fn tool_name(raw: &str) -> String {
    let ok = !raw.is_empty()
        && raw.len() <= NAME_MOST
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        raw.to_string()
    } else {
        "?".to_string()
    }
}

/// One tool result a request carried back to the model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolResult {
    /// The tool's name, from the call it answers ("?" when the call is not in the message before it).
    pub tool: String,
    /// The result's `is_error` was true.
    pub error: bool,
}

/// What an answer said, read as it streamed past: the tools it called, in order, and its stop reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub called: Vec<String>,
    pub stop: String,
}

/// One model step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// The tools the answer called, in order.
    pub called: Vec<String>,
    /// The tool results in the request's last user message.
    pub results: Vec<ToolResult>,
    /// The answer's `stop_reason` ("" when it named none).
    pub stop: String,
    /// The HTTP status Claude Code got.
    pub status: u16,
    /// From the request's arrival at the relay to the answer's first byte (None: no answer came).
    pub first_byte_ms: Option<i64>,
    /// From the request's arrival to the answer's end.
    pub done_ms: i64,
}

impl Step {
    fn as_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("called".into(), json!(self.called));
        m.insert(
            "results".into(),
            Value::Array(
                self.results
                    .iter()
                    .map(|r| json!({"tool": r.tool, "error": r.error}))
                    .collect(),
            ),
        );
        if !self.stop.is_empty() {
            m.insert("stop".into(), json!(self.stop));
        }
        if !(200..300).contains(&self.status) {
            m.insert("status".into(), json!(self.status));
        }
        if let Some(ms) = self.first_byte_ms {
            m.insert("first_byte_ms".into(), json!(ms));
        }
        m.insert("done_ms".into(), json!(self.done_ms));
        Value::Object(m)
    }
}

/// A run's steps since they were last taken: the latest [`KEPT`], and how many earlier ones were let go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Steps {
    pub kept: VecDeque<Step>,
    pub dropped: i64,
}

impl Steps {
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty() && self.dropped == 0
    }

    pub fn push(&mut self, step: Step) {
        if self.kept.len() >= KEPT {
            self.kept.pop_front();
            self.dropped += 1;
        }
        self.kept.push_back(step);
    }

    /// Adds the steps that came after (a run's steps stashed at its cancel and what followed).
    pub fn add(&mut self, later: Steps) {
        self.dropped += later.dropped;
        for s in later.kept {
            self.push(s);
        }
    }

    /// Adds `steps` (and `steps_dropped` when any were let go) to a usage record; nothing when there are none.
    pub fn insert_into(&self, data: &mut Map<String, Value>) {
        if self.is_empty() {
            return;
        }
        data.insert(
            "steps".into(),
            Value::Array(self.kept.iter().map(Step::as_json).collect()),
        );
        if self.dropped > 0 {
            data.insert("steps_dropped".into(), json!(self.dropped));
        }
    }
}

/// The tool results in a request body's last user message, each named from the call it answers in the assistant
/// message just before it. None when the body is not a JSON request. Everything but types, ids, names and error flags
/// is skipped unread into memory (a screenshot's bytes are passed over, not copied).
pub fn results(body: &[u8]) -> Option<Vec<ToolResult>> {
    let request: Request = serde_json::from_slice(body).ok()?;
    let messages = request.messages;
    let Some(last) = messages.iter().rposition(|m| m.role == "user") else {
        return Some(Vec::new());
    };
    let names: HashMap<&str, &str> = match last.checked_sub(1).map(|i| &messages[i]) {
        Some(before) if before.role == "assistant" => before
            .content
            .iter()
            .filter(|b| b.kind == "tool_use")
            .filter_map(|b| Some((b.id.as_deref()?, b.name.as_deref()?)))
            .collect(),
        _ => HashMap::new(),
    };
    Some(
        messages[last]
            .content
            .iter()
            .filter(|b| b.kind == "tool_result")
            .map(|b| ToolResult {
                tool: b
                    .tool_use_id
                    .as_deref()
                    .and_then(|id| names.get(id))
                    .map_or_else(|| "?".to_string(), |n| tool_name(n)),
                error: b.is_error == Some(Value::Bool(true)),
            })
            .collect(),
    )
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default, deserialize_with = "any_string")]
    role: String,
    #[serde(default, deserialize_with = "blocks")]
    content: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type", default, deserialize_with = "any_string")]
    kind: String,
    #[serde(default, deserialize_with = "some_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "some_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "some_string")]
    tool_use_id: Option<String>,
    #[serde(default)]
    is_error: Option<Value>,
}

/// A string field; any other value reads as "" (and is skipped, not kept).
fn any_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(some_string(d)?.unwrap_or_default())
}

fn some_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("any value")
        }
        fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
            Ok(Some(s.to_string()))
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(None)
        }
    }
    d.deserialize_any(V)
}

/// A message's `content`: its blocks when it is a list (an entry that is not an object reads as an empty block); a
/// plain string (or anything else) has none.
fn blocks<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Block>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<Block>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a message's content")
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(Vec::new())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(b) = seq.next_element::<MaybeBlock>()? {
                out.extend(b.0);
            }
            Ok(out)
        }
    }
    d.deserialize_any(V)
}

/// A content entry: a block when it is an object, skipped otherwise.
struct MaybeBlock(Option<Block>);

impl<'de> Deserialize<'de> for MaybeBlock {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = MaybeBlock;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a content block")
            }
            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Block::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(|b| MaybeBlock(Some(b)))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(MaybeBlock(None))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(MaybeBlock(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(MaybeBlock(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(MaybeBlock(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(MaybeBlock(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(MaybeBlock(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(MaybeBlock(None))
            }
        }
        d.deserialize_any(V)
    }
}
