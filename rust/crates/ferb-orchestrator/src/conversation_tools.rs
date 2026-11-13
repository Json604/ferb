//! ferb/orchestrator/conversation_tools.py: `conversation.read`, read-only delegated-conversation evidence scoped to
//! the requesting task tree. Its run, on validated arguments.

use serde_json::{json, Value};

use crate::conversations::ConversationStore;
use crate::evidence::Answer;
use crate::sql::Result;

/// `conversation.read`'s validated arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadArgs {
    pub conversation_id: Option<String>,
    pub offset: usize,
    pub max_chars: usize,
}

/// conversation_tools.py:27 `ConversationRead.run`. `root`: the caller's tree root, when known.
pub fn conversation_read(
    store: &ConversationStore<'_>,
    root: Option<String>,
    task_id: &str,
    args: &ReadArgs,
) -> Result<Answer> {
    let root = root
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| task_id.split('.').next().unwrap_or("").to_string());
    let rows = store.list(Some(&root))?;
    let id = args.conversation_id.as_deref().filter(|c| !c.is_empty());
    let data = match id {
        Some(c) => {
            if !rows
                .iter()
                .any(|r| r.get("id").and_then(|v| v.as_str()) == Some(c))
            {
                return Ok(Answer {
                    content: json!({"error": "No conversation with that ID in this task tree"}),
                    is_error: true,
                    read_sources: Vec::new(),
                });
            }
            let mut d = store.digest(c)?;
            d["observations"] = Value::Array(store.events(c, 0)?);
            d
        }
        None => json!({"conversations": rows.into_iter().map(Value::Object).collect::<Vec<_>>()}),
    };
    let outside = id.is_some()
        && (data.get("observations").is_some_and(ferb_py::value::truthy)
            || data
                .get("turns")
                .and_then(|t| t.as_array())
                .is_some_and(|ts| {
                    ts.iter()
                        .any(|t| t.get("output").is_some_and(ferb_py::value::truthy))
                }));
    let text_ = ferb_py::json::dumps_utf8(&data);
    let chars: Vec<char> = text_.chars().collect();
    let end = args.offset + args.max_chars;
    let shown: String = chars
        .iter()
        .skip(args.offset)
        .take(end.saturating_sub(args.offset))
        .collect();
    let read_sources = if outside {
        vec![
            ("origin".to_string(), "external:conversation".to_string()),
            ("origin".to_string(), "private".to_string()),
        ]
    } else {
        vec![("origin".to_string(), "inferred".to_string())]
    };
    Ok(Answer {
        content: json!({"text": shown, "next_offset": if end < chars.len() { json!(end) } else { Value::Null },
                        "total_chars": chars.len()}),
        is_error: false,
        read_sources,
    })
}
