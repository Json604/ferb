//! Model-visible tool names from a Codex request trace line (ferb/runtimes/tooltrace.py): each request's
//! `"tools":[...]` and every `tool_search` result (logged as `output=[...]`). Nameless hosted tools are kept by type:
//! a tool the check can't see is exactly what it must never miss.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::Raised;
use crate::pyjson::raw_decode;

const MARKERS: [&str; 2] = ["\"tools\":", " output="];

/// tooltrace.py `tools_in`. `Err` where Python would raise (a namespace without a name, a name that isn't text).
pub fn tools_in(text: &str) -> Result<BTreeSet<String>, Raised> {
    let mut names = BTreeSet::new();
    let mut i = 0usize;
    loop {
        let mut best: Option<(usize, &str)> = None;
        for m in MARKERS {
            let needle = format!("{m}[");
            if let Some(p) = text.get(i..).and_then(|t| t.find(&needle)) {
                let pos = i + p;
                // min() over (position, marker): the smaller marker text wins a tie
                if best.is_none_or(|(bp, bm)| (pos, m) < (bp, bm)) {
                    best = Some((pos, m));
                }
            }
        }
        let Some((pos, marker)) = best else {
            return Ok(names);
        };
        match raw_decode(text, pos + marker.len()) {
            None => {
                i = next_char(text, pos);
                continue;
            }
            Some((arr, end)) => {
                if marker == "\"tools\":" || looks_like_tools(&arr) {
                    names.extend(flatten(&arr)?);
                    i = end;
                } else {
                    i = next_char(text, pos);
                }
            }
        }
    }
}

fn next_char(text: &str, pos: usize) -> usize {
    pos + text[pos..].chars().next().map_or(1, char::len_utf8)
}

fn looks_like_tools(arr: &Value) -> bool {
    match arr {
        Value::Array(items) => {
            !items.is_empty()
                && items.iter().all(|t| {
                    matches!(
                        t.get("type").and_then(Value::as_str),
                        Some("function" | "namespace" | "custom")
                    )
                })
        }
        _ => false,
    }
}

fn flatten(tools: &Value) -> Result<BTreeSet<String>, Raised> {
    let mut out = BTreeSet::new();
    let items: Vec<&Value> = match tools {
        Value::Array(items) => items.iter().collect(),
        // iterating a dict or a str yields keys or characters: never a dict, so nothing
        Value::Object(_) | Value::String(_) => return Ok(out),
        other => {
            return Err(Raised::new(
                "TypeError",
                format!("'{}' object is not iterable", crate::error::py_type(other)),
            ))
        }
    };
    for t in items {
        let Value::Object(map) = t else { continue };
        let nested = map.get("tools");
        if nested.is_some_and(ferb_py::value::truthy) {
            let name = map.get("name").ok_or_else(|| Raised::key_error("name"))?;
            let name = ferb_py::value::py_str(name);
            for n in flatten(nested.unwrap_or(&Value::Null))? {
                out.insert(format!("{name}.{n}"));
            }
        } else {
            let chosen = [map.get("name"), map.get("type")]
                .into_iter()
                .flatten()
                .find(|v| ferb_py::value::truthy(v));
            match chosen {
                Some(Value::String(s)) => {
                    out.insert(s.clone());
                }
                Some(other) => {
                    // a name that isn't text: Python's `removeprefix` on it raises in the caller
                    return Err(Raised::new(
                        "AttributeError",
                        format!(
                            "'{}' object has no attribute 'removeprefix'",
                            crate::error::py_type(other)
                        ),
                    ));
                }
                None => {
                    out.insert(ferb_py::json::dumps(t));
                }
            }
        }
    }
    Ok(out)
}
