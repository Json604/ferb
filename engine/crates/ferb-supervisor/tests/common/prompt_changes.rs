//! The deliberate prompt changes made after the Python fixtures were recorded (ENG-6), applied to a recorded text
//! before it is compared with what the port builds from engine/runtime/orchestrator/prompts/*.md. The changes are listed, each
//! with its decision, in engine/fixtures/catalog/prompt_changes.json; every loader of a recorded prompt text (the
//! catalog's prompt_static.json, the bodies of engine/fixtures/daemon and engine/fixtures/supervisor) calls `apply`.
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

/// What a change's text becomes after a later change (`old` → `new`): that change applied to it when its old text is
/// inside it (ORCH-663 on ORCH-640's line), or that change's new text when its old text holds it whole (PRM-373 took
/// ORCH-663's lookup line out with the sentence around it); else unchanged.
pub fn became_after(text: &str, old: &str, new: &str) -> String {
    if text.contains(old) {
        text.replace(old, new)
    } else if old.contains(text) {
        new.to_string()
    } else {
        text.to_string()
    }
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
    let all = changes();
    all.iter()
        .enumerate()
        .filter(|(n, (_, _, new))| {
            // a later change may change this one's text again (ORCH-663 on ORCH-640's line), or replace it whole
            // (PRM-373 on ORCH-663's lookup line): look for what it became
            let mut became = new.clone();
            for (_, o, w) in &all[n + 1..] {
                became = became_after(&became, o, w);
            }
            new_forms(&became)
                .iter()
                .any(|f| system.contains(f.as_str()))
        })
        .map(|(_, (_, old, new))| new.len() as i64 - old.len() as i64)
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
/// `about.diagnostics`, ORCH-663 `task.trace`): (name, the tool it follows, its listed row as `tools_for` writes it,
/// whether it is offered only to a grant naming it: the planner's lookups).
pub fn tool_additions() -> &'static [(String, String, Value, bool)] {
    static A: OnceLock<Vec<(String, String, Value, bool)>> = OnceLock::new();
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
                let named_only = t["facts"]["named_only"].as_bool().unwrap_or(false);
                (s("name"), s("after"), Value::Object(row), named_only)
            })
            .collect()
    })
}

/// How many more tokens a model call reserves when the Rust engine's own tools are offered beside the recorded ones
/// (decision: DMN-595): `reserve_tokens` counts the listing as `json.dumps` writes it, so each addition offered (its
/// recorded neighbour `after` was) adds its row's bytes and the ", " before it. 0 for a call offered no tools.
pub fn tool_shift(recorded_tools: &[String]) -> i64 {
    // HAND-642: an addition may follow another addition (project.fetch after project.publish): the listing grows as
    // each one offered joins it
    let mut listed: Vec<String> = recorded_tools.to_vec();
    let mut offered: Vec<(String, Value)> = Vec::new();
    for (name, after, row, named_only) in tool_additions() {
        let named_only = *named_only;
        let joins = listed.iter().any(|t| t == after)
            && !recorded_tools.iter().any(|t| t == name)
            && (!named_only || planner_call(recorded_tools))
            && ferb_gateway::catalog::additions()
                .iter()
                .find(|a| a.name == *name)
                .is_none_or(|a| ferb_gateway::catalog::offered_beside(a, recorded_tools));
        if joins {
            listed.push(name.clone());
            // one that follows another addition: the filter below counts only those after a recorded tool
            if ferb_gateway::catalog::is_addition(after) {
                offered.push((name.clone(), row.clone()));
            }
        }
    }
    let chained: i64 = offered
        .iter()
        .map(|(_, row)| ferb_py::json::dumps(row).len() as i64 + 2)
        .sum();
    chained
        + tool_additions()
            .iter()
            .filter(|(name, after, _, named_only)| {
                // SENSE-422: a planner lookup with no recorded neighbour (device.facts) is offered to every planner call
                (recorded_tools.iter().any(|t| t == after)
                || (planner_call(recorded_tools)
                    && ferb_orchestrator::lookups::PLANNER_TOOLS.contains(&name.as_str())))
                && !recorded_tools.iter().any(|t| t == name)
                && (!named_only || planner_call(recorded_tools))
                // HAND-623: an acting, owner-only addition only where its grant would be offered it
                && ferb_gateway::catalog::additions()
                    .iter()
                    .find(|a| a.name == *name)
                    .is_none_or(|a| ferb_gateway::catalog::offered_beside(a, recorded_tools))
            })
            .map(|(_, _, row, _)| ferb_py::json::dumps(row).len() as i64 + 2)
            .sum::<i64>()
        + tool_changes_shift(recorded_tools)
}

/// Python's planner lookups as the recordings offered them (supervisor.py `PLANNER_TOOLS`, frozen with them).
const RECORDED_LOOKUPS: [&str; 6] = [
    "memory.read",
    "chat.history",
    "task.evidence",
    "trail.query",
    "trail.days",
    "about.read",
];

/// A recorded call offered only the planner's lookups: the one grant that names its tools, so the one offered a tool
/// that only a grant naming it gets (ORCH-663).
pub fn planner_call(recorded_tools: &[String]) -> bool {
    !recorded_tools.is_empty()
        && recorded_tools.iter().all(|t| {
            RECORDED_LOOKUPS
                .iter()
                .any(|l| t == l || *t == l.replace('.', "_"))
        })
}

/// The deliberate changes to Python's tools (engine/fixtures/catalog/tool_changes.json, each with its decision; HAND-551
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
        // the tools the line names, by their catalog names (HAND-623: which grant this listing came from)
        let rows = ferb_gateway::catalog::catalog().raw()["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let dotted: Vec<String> = names
            .iter()
            .filter_map(|n| rows.iter().find(|t| t["mcp_name"] == n.as_str()))
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        // the worker's line: a tool offered only by name (the planner's) is never in it
        for (name, after, _, _) in tool_additions().iter().filter(|a| !a.3) {
            let offered = ferb_gateway::catalog::additions()
                .iter()
                .find(|a| a.name == *name)
                .is_none_or(|a| ferb_gateway::catalog::offered_beside(a, &dotted));
            if !offered {
                continue;
            }
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
