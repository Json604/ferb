//! `lineage.py`: the source refs FERB's structured readers name (a planner's `entry:`/`task:` handles, a tool call's
//! result and paged evidence). Never parses prose.

use std::collections::BTreeSet;

use serde_json::Value;

pub const ENTRY: &str = "entry:";
pub const TASK: &str = "task:";
/// The planner block that carries the owner's message (Layer 5).
pub const REQUEST_REFERENCE: &str = "current request";

/// lineage.py:16 `direct`.
pub fn direct(r: &str) -> String {
    if let Some(rest) = r.strip_prefix(ENTRY) {
        return format!("journal:{rest}");
    }
    if let Some(rest) = r.strip_prefix(TASK) {
        return format!("episode:{rest}");
    }
    r.to_owned()
}

fn obj(v: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    v.and_then(Value::as_object)
}

fn list(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// `str(x)` of a ref read from JSON.
fn text(v: &Value) -> String {
    crate::py::py_str(v)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// lineage.py:26 `record_sources`.
pub fn record_sources(record: &Value) -> BTreeSet<String> {
    let empty = serde_json::Map::new();
    let inp = record
        .get("input")
        .filter(|v| ferb_py::value::truthy(v))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    match record.get("kind").and_then(Value::as_str) {
        Some("context.source") | Some("context.exposure") => {
            list(inp.get("source_refs")).iter().map(text).collect()
        }
        Some("tool.call") => call_sources(&Value::Object(inp.clone()), record.get("output")),
        _ => BTreeSet::new(),
    }
}

/// lineage.py:35 `call_sources`.
pub fn call_sources(inp: &Value, result: Option<&Value>) -> BTreeSet<String> {
    let tool = inp.get("tool").and_then(Value::as_str);
    let empty = serde_json::Map::new();
    let args = obj(inp.get("args").filter(|v| ferb_py::value::truthy(v))).unwrap_or(&empty);
    let result = obj(result).unwrap_or(&empty);
    let mut refs = BTreeSet::new();
    match tool {
        Some(t @ ("memory.read" | "memory.learned")) => {
            let prefix = if t == "memory.read" { "memory:" } else { "" };
            if truthy(args.get("item_id")) {
                refs.insert(format!("{prefix}{}", text(&args["item_id"])));
            }
            refs.extend(list(result.get("source_refs")).iter().map(text));
            let mut pages = vec![result.clone()];
            if let Some(Value::String(t)) = result.get("text") {
                if let Ok(Value::Object(full)) = ferb_py::json::loads(t) {
                    pages.push(full);
                }
            }
            for page in &pages {
                let entries = page.get("items").or_else(|| page.get("candidates"));
                for e in list(entries) {
                    if let Some(e) = e.as_object() {
                        if truthy(e.get("id")) {
                            refs.insert(format!("{prefix}{}", text(&e["id"])));
                        }
                    }
                }
            }
        }
        Some("trail.query") => {
            if truthy(args.get("event_id")) {
                refs.insert(format!("trail:{}", text(&args["event_id"])));
            }
            for e in list(result.get("events")) {
                if truthy(e.get("id")) {
                    refs.insert(format!("trail:{}", text(&e["id"])));
                }
            }
        }
        Some("trail.days") => {
            for e in list(result.get("days")) {
                if truthy(e.get("id")) {
                    refs.insert(format!("journal:{}", text(&e["id"])));
                }
            }
            refs.extend(list(result.get("source_refs")).iter().map(text));
        }
        Some("task.evidence") => {
            refs.extend(list(result.get("source_refs")).iter().map(text));
            for record in list(result.get("records")) {
                refs.extend(record_sources(record));
            }
            if let Some(Value::String(t)) = result.get("text") {
                if let Ok(full @ Value::Object(_)) = ferb_py::json::loads(t) {
                    refs.extend(record_sources(&full));
                }
            }
        }
        _ => {}
    }
    refs
}
