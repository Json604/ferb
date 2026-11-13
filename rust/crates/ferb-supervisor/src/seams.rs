//! What the composition reads that another part keeps: [`JournalRow`], a journal row as the composition reads it (the
//! journal itself is ferb-learning's). The flows' seams are [`crate::flow::Daemon`] and ferb-worker's runtime
//! interface (`ModelRuntime`, `EventSource`, `RunControl`).

use std::collections::BTreeSet;

use ferb_clock::py::Time;
use serde_json::{Map, Value};

/// One journal row (ferb/memory/journal.py `JournalEntry`), its time parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct JournalRow {
    pub id: String,
    pub at: Time,
    pub kind: String,
    pub data: Map<String, Value>,
    pub origin: String,
}

impl JournalRow {
    /// A row as rust/fixtures/supervisor records it: `{id, at (ISO), kind, data, origin}`.
    pub fn from_json(v: &Value) -> Option<JournalRow> {
        Some(JournalRow {
            id: v.get("id")?.as_str()?.to_string(),
            at: ferb_clock::py::parse(v.get("at")?.as_str()?)?,
            kind: v.get("kind")?.as_str()?.to_string(),
            data: v.get("data")?.as_object()?.clone(),
            origin: v
                .get("origin")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }
}

/// The planner's origin sets, written as Python sorts them in a record.
pub fn origins<'a>(items: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    items.into_iter().map(str::to_string).collect()
}
