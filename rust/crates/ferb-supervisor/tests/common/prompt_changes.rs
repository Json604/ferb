//! The deliberate prompt changes made after the Python fixtures were recorded (ENG-6), applied to a recorded text
//! before it is compared with what the port builds from ferb/orchestrator/prompts/*.md. The changes are listed, each
//! with its decision, in rust/fixtures/catalog/prompt_changes.json; every loader of a recorded prompt text (the
//! catalog's prompt_static.json, the bodies of rust/fixtures/daemon and rust/fixtures/supervisor) calls `apply`.
//! Included by path from other test crates (ferbd), so it uses only std, serde_json and ferb-py.
#![allow(dead_code)]

use std::sync::OnceLock;

use serde_json::Value;

/// (decision, old, new) in order.
pub fn changes() -> &'static [(String, String, String)] {
    static C: OnceLock<Vec<(String, String, String)>> = OnceLock::new();
    C.get_or_init(|| {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/catalog/prompt_changes.json");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: Value = serde_json::from_str(&text).expect("prompt_changes.json");
        v["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .map(|c| {
                let s = |k: &str| c[k].as_str().expect(k).to_string();
                (s("decision"), s("old"), s("new"))
            })
            .collect()
    })
}

/// A text's JSON-escaped form without its quotes, as Python's `json.dumps` writes it (`ascii`) or with
/// `ensure_ascii=False`.
fn escaped(text: &str, ascii: bool) -> String {
    let dumped = if ascii {
        ferb_py::json::dumps(&Value::String(text.to_string()))
    } else {
        ferb_py::json::dumps_utf8(&Value::String(text.to_string()))
    };
    dumped[1..dumped.len() - 1].to_string()
}

/// The recorded text with every change applied: raw, JSON-escaped once (a prompt inside a recorded JSON text) and
/// twice (inside a JSON text that is itself a string of a recorded row).
pub fn apply(text: &str) -> String {
    let mut out = text.to_string();
    for (_, old, new) in changes() {
        let mut forms = vec![(old.clone(), new.clone())];
        for ascii in [true, false] {
            let once = (escaped(old, ascii), escaped(new, ascii));
            let twice = (escaped(&once.0, ascii), escaped(&once.1, ascii));
            forms.push(once);
            forms.push(twice);
        }
        for (o, n) in forms {
            if out.contains(&o) {
                out = out.replace(&o, &n);
            }
        }
    }
    out
}

/// The forms a changed prompt's new text takes in a recorded text: raw and JSON-escaped once or twice.
fn new_forms(new: &str) -> Vec<String> {
    let mut forms = vec![new.to_string()];
    for ascii in [true, false] {
        let once = escaped(new, ascii);
        forms.push(escaped(&once, ascii));
        forms.push(once);
    }
    forms
}

/// How many more tokens a model call reserves for a system holding the changes (`reserve_tokens` counts the
/// system): the recordings all count UTF-8 bytes (`utf8_byte_ceiling_tokenizer_unavailable`), so each change adds
/// its new text's bytes less its old text's.
pub fn reserved_shift(system: &str) -> i64 {
    changes()
        .iter()
        .filter(|(_, _, new)| new_forms(new).iter().any(|f| system.contains(f.as_str())))
        .map(|(_, old, new)| new.len() as i64 - old.len() as i64)
        .sum()
}

/// Every context budget line in a recorded text with its `reserved_tokens` moved by `by` (the line raw or
/// JSON-escaped). Only the recordings' byte count is known here: any other method fails the test.
pub fn shift_reserved(text: &str, by: i64) -> String {
    if by == 0 {
        return text.to_string();
    }
    let key = "reserved_tokens";
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(key) {
        let after = &rest[at + key.len()..];
        let quote = after.len() - after.trim_start_matches(['\\', '"']).len();
        let tail = &after[quote..];
        let Some(digits_at) = tail.strip_prefix(": ") else {
            out.push_str(&rest[..at + key.len()]);
            rest = after;
            continue;
        };
        let n = digits_at.len()
            - digits_at
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .len();
        let value: i64 = digits_at[..n].parse().expect("reserved_tokens' number");
        let line_end = digits_at.find('}').unwrap_or(digits_at.len());
        assert!(
            digits_at[..line_end].contains("utf8_byte_ceiling_tokenizer_unavailable"),
            "a recorded budget line counted another way: {}",
            &digits_at[..line_end]
        );
        out.push_str(&rest[..at + key.len() + quote + 2]);
        out.push_str(&(value + by).to_string());
        rest = &digits_at[n..];
    }
    out.push_str(rest);
    out
}

/// A recorded text that holds a changed system (a model turn's span input) with its own budget lines moved to it.
pub fn shift_with_system(text: &str) -> String {
    shift_reserved(text, reserved_shift(text))
}

/// `apply` to every string inside a recorded value.
pub fn apply_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(apply(s)),
        Value::Array(a) => Value::Array(a.iter().map(apply_value).collect()),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), apply_value(v))).collect())
        }
        other => other.clone(),
    }
}

/// The Rust engine's own tools (ferb-gateway's `catalog_additions.json`, each with its decision; DMN-595
/// `about.diagnostics`): (name, the tool it follows, its listed row as `tools_for` writes it).
pub fn tool_additions() -> &'static [(String, String, Value)] {
    static A: OnceLock<Vec<(String, String, Value)>> = OnceLock::new();
    A.get_or_init(|| {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ferb-gateway/src/catalog_additions.json");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: Value = serde_json::from_str(&text).expect("catalog_additions.json");
        v["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| {
                let mut row = serde_json::Map::new();
                for k in ["name", "description", "input_schema"] {
                    row.insert(k.into(), t[k].clone());
                }
                let s = |k: &str| t[k].as_str().expect(k).to_string();
                (s("name"), s("after"), Value::Object(row))
            })
            .collect()
    })
}

/// How many more tokens a model call reserves when the Rust engine's own tools are offered beside the recorded ones
/// (decision: DMN-595): `reserve_tokens` counts the listing as `json.dumps` writes it, so each addition offered (its
/// recorded neighbour `after` was) adds its row's bytes and the ", " before it. 0 for a call offered no tools.
pub fn tool_shift(recorded_tools: &[String]) -> i64 {
    tool_additions()
        .iter()
        .filter(|(name, after, _)| {
            recorded_tools.iter().any(|t| t == after) && !recorded_tools.iter().any(|t| t == name)
        })
        .map(|(_, _, row)| ferb_py::json::dumps(row).len() as i64 + 2)
        .sum::<i64>()
        + tool_changes_shift(recorded_tools)
}

/// The deliberate changes to Python's tools (rust/fixtures/catalog/tool_changes.json, each with its decision; HAND-551
/// mac.act's element actions): (tool, part, old, new).
pub fn tool_changes() -> &'static [(String, String, String, String)] {
    static C: OnceLock<Vec<(String, String, String, String)>> = OnceLock::new();
    C.get_or_init(|| {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/catalog/tool_changes.json");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let v: Value = serde_json::from_str(&text).expect("tool_changes.json");
        v["changes"]
            .as_array()
            .expect("changes")
            .iter()
            .map(|c| {
                let s = |k: &str| c[k].as_str().expect(k).to_string();
                (s("tool"), s("part"), s("old"), s("new"))
            })
            .collect()
    })
}

/// How many more tokens a model call offered a changed tool reserves (decision: HAND-551): the listing's bytes grow by
/// each change's new text less its old one, as `json.dumps` writes them.
pub fn tool_changes_shift(recorded_tools: &[String]) -> i64 {
    tool_changes()
        .iter()
        .filter(|(tool, part, _, _)| {
            part != "validation"
                && recorded_tools
                    .iter()
                    .any(|t| *t == *tool || *t == tool.replace('.', "_"))
        })
        .map(|(_, part, old, new)| {
            // a schema piece is JSON text inside the listing as it is; a description is a string, escaped
            let d = |t: &str| {
                if part == "input_schema" {
                    t.len() as i64
                } else {
                    ferb_py::json::dumps(&Value::String(t.to_string())).len() as i64
                }
            };
            d(new) - d(old)
        })
        .sum()
}

/// The worker's line naming its FERB tools (`Your FERB tools (MCP server `ferb`): a, b, c.`) with the Rust engine's
/// own tools named after the tool each follows, wherever that one is named (decision: DMN-595). Returns the text and
/// how many bytes were added.
pub fn with_tool_names(text: &str) -> (String, i64) {
    const LINE: &str = "Your FERB tools (MCP server `ferb`): ";
    let mut out = String::new();
    let mut rest = text;
    let mut added = 0i64;
    while let Some(at) = rest.find(LINE) {
        let start = at + LINE.len();
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail.find('.').unwrap_or(tail.len());
        let mut names: Vec<String> = tail[..end].split(", ").map(str::to_string).collect();
        for (name, after, _) in tool_additions() {
            let (name, after) = (name.replace('.', "_"), after.replace('.', "_"));
            if let Some(i) = names.iter().position(|n| *n == after) {
                if !names.contains(&name) {
                    names.insert(i + 1, name.clone());
                    added += name.len() as i64 + 2;
                }
            }
        }
        out.push_str(&names.join(", "));
        rest = &tail[end..];
    }
    out.push_str(rest);
    (out, added)
}
