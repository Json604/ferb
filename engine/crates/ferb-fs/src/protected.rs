//! The never-read folders in or around a path a file tool walked or listed, as a fact for the worker (owner,
//! 2026-11-16: "ferb should be able to say this that it can't read this and why"; HAND-595, HAND-597).
//!
//! The protection lists reach the file tools through one seam, `ToolContext.path_closed` (path, access). Beside the
//! accesses that ask whether a call may touch a path, the gateway answers [`WITHIN`]: the never-read folders that are
//! the path, lie inside it or hold it, minus those the owner let this task read, as a JSON array of
//! `{"rule", "why"}`. A check that doesn't know the word answers None (or text that isn't that array): no fact, and
//! the result's bytes are Python's.

use serde_json::{json, Value};

use ferb_core::types::ToolContext;

/// The query word on `path_closed`.
pub const WITHIN: &str = "within";
/// At most this many folders are listed; `more` counts the rest.
pub const CAP: usize = 12;

pub const DISK_USAGE_FACT: &str =
    "These protected folders (the owner's never-read list) are this path, inside it or \
hold it. Their files' sizes are in this total (metadata only); what is in them was not read.";
pub const LIST_FACT: &str =
    "These protected folders (the owner's never-read list) are this folder or under it: names \
show in listings, but FERB's tools don't read inside them without the owner's Yes for this task.";
pub const ALLOW: &str = "Only the owner can open one: name it in an fs call (FERB asks the owner once per folder for \
this task; a Yes lets this task read it), or for a shell command set escape to its rule; or report it as not read \
and why.";

/// The gateway's answer to [`WITHIN`] for `path`: (rule, why) pairs.
pub fn within(ctx: &ToolContext, path: &str) -> Vec<(String, String)> {
    let Some(text) = ctx
        .path_closed
        .as_ref()
        .and_then(|c| (c.0)(std::path::Path::new(path), WITHIN))
    else {
        return Vec::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|i| {
            Some((
                i.get("rule")?.as_str()?.to_string(),
                i.get("why")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// The `not_read` fact for `path` (None when no never-read folder is in or around it): the fact, how one is opened,
/// at most [`CAP`] folders (path, rule, why) and how many more.
// decision: HAND-597
pub fn fact(ctx: &ToolContext, path: &str, fact: &str) -> Option<Value> {
    let found = within(ctx, path);
    if found.is_empty() {
        return None;
    }
    let listed: Vec<Value> = found
        .iter()
        .take(CAP)
        .map(|(rule, why)| {
            json!({"path": rule.strip_prefix("read:").unwrap_or(rule), "rule": rule, "why": why})
        })
        .collect();
    let mut out = json!({"fact": fact, "allow": ALLOW, "folders": listed});
    if found.len() > CAP {
        out["more"] = json!(found.len() - CAP);
    }
    Some(out)
}
