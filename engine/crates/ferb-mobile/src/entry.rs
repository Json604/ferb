//! One chat entry as the engine sends it (ENG-8, `ferb_core::chat_entry`): `{id, conversation_id, pos, seq, rev, kind,
//! state, open, request_id?, task_id?, approval_id?, ref?, answers?, placement?, origin?, client_id?, submission_id?,
//! created_at, updated_at, day, body, deleted}`. Read tolerantly as FERB.app reads it (`ChatEntry.init`, APP-801): an
//! entry without its id, kind or order is left out; a field of the wrong type reads as absent.

use serde_json::{Map, Value};

/// Where a stream's own entries start (ENG-12): below it, history carried from `chat_history` (`legacy`).
pub const STREAM_POS_START: i64 = 1 << 32;

#[derive(Debug, Clone, PartialEq)]
pub struct ChatEntry {
    pub id: String,
    pub conversation_id: String,
    pub pos: i64,
    pub seq: i64,
    pub rev: i64,
    /// message, plan, answer, question, notice, post, error or legacy.
    pub kind: String,
    pub state: String,
    pub open: bool,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub approval_id: Option<String>,
    pub r#ref: Option<String>,
    pub answers: Option<String>,
    pub placement: Option<String>,
    pub origin: Option<String>,
    pub client_id: Option<String>,
    pub submission_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub day: String,
    pub body: Map<String, Value>,
    pub deleted: bool,
    /// The reply cut this entry's words to fit (`body_cut {chars, full_chars}`, ENG-14): never folded or stored in
    /// place of a whole entry; `chat.entries.get` reads it whole.
    pub cut: bool,
}

/// A JSON whole number as Swift's `NSNumber.intValue` reads it (a float is cut toward zero, a bool is 0 or 1).
pub fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

impl ChatEntry {
    /// Read tolerantly: None when it has no id, kind or pos.
    pub fn read(value: &Value) -> Option<ChatEntry> {
        let v = value.as_object()?;
        let words = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let id = words("id").filter(|s| !s.is_empty())?;
        let kind = words("kind")?;
        let pos = int(v.get("pos")?)?;
        Some(ChatEntry {
            id,
            conversation_id: words("conversation_id").unwrap_or_default(),
            pos,
            seq: v.get("seq").and_then(int).unwrap_or(0),
            rev: v.get("rev").and_then(int).unwrap_or(1),
            kind,
            state: words("state").unwrap_or_default(),
            open: v.get("open").and_then(Value::as_bool).unwrap_or(false),
            request_id: words("request_id"),
            task_id: words("task_id"),
            approval_id: words("approval_id"),
            r#ref: words("ref"),
            answers: words("answers"),
            placement: words("placement"),
            origin: words("origin"),
            client_id: words("client_id"),
            submission_id: words("submission_id"),
            created_at: words("created_at").unwrap_or_default(),
            updated_at: words("updated_at").unwrap_or_default(),
            day: words("day").unwrap_or_default(),
            body: v
                .get("body")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            deleted: v.get("deleted").and_then(Value::as_bool).unwrap_or(false),
            cut: v.get("body_cut").is_some_and(|c| !c.is_null()),
        })
    }

    /// The entries of a JSON list (each revision of an entry may be among them).
    pub fn list(value: &Value) -> Vec<ChatEntry> {
        value
            .as_array()
            .map(|a| a.iter().filter_map(ChatEntry::read).collect())
            .unwrap_or_default()
    }

    /// The entry as JSON again (what the store keeps), in the engine's field names.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        let opt = |s: &Option<String>| s.clone().map(Value::String).unwrap_or(Value::Null);
        m.insert("id".into(), self.id.clone().into());
        m.insert(
            "conversation_id".into(),
            self.conversation_id.clone().into(),
        );
        m.insert("pos".into(), self.pos.into());
        m.insert("seq".into(), self.seq.into());
        m.insert("rev".into(), self.rev.into());
        m.insert("kind".into(), self.kind.clone().into());
        m.insert("state".into(), self.state.clone().into());
        m.insert("open".into(), self.open.into());
        m.insert("request_id".into(), opt(&self.request_id));
        m.insert("task_id".into(), opt(&self.task_id));
        m.insert("approval_id".into(), opt(&self.approval_id));
        m.insert("ref".into(), opt(&self.r#ref));
        m.insert("answers".into(), opt(&self.answers));
        m.insert("placement".into(), opt(&self.placement));
        m.insert("origin".into(), opt(&self.origin));
        m.insert("client_id".into(), opt(&self.client_id));
        m.insert("submission_id".into(), opt(&self.submission_id));
        m.insert("created_at".into(), self.created_at.clone().into());
        m.insert("updated_at".into(), self.updated_at.clone().into());
        m.insert("day".into(), self.day.clone().into());
        m.insert("body".into(), Value::Object(self.body.clone()));
        m.insert("deleted".into(), self.deleted.into());
        Value::Object(m)
    }

    /// Whether `self` is a newer copy than `other` of the same entry: a higher `rev`, at the same `rev` a higher
    /// `seq` (the engine bumps both together, so equal pairs are the same revision). Order-independent, so entries
    /// arriving in any order keep the same newest copy.
    pub fn newer_than(&self, other: &ChatEntry) -> bool {
        (self.rev, self.seq) > (other.rev, other.seq)
    }
}
