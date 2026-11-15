//! `mac_profile.py` (MEM-13, MEM-426): what FERB works out about the owner from their Mac. The facts are gathered
//! natively (`ferb/device/persona.py`, a seam: they arrive here as JSON); one model call interprets them; code stores
//! each conclusion as an observed item resting on a `memory.mac_profile` journal row holding the facts.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use ferb_context::PromptRoot;
use ferb_memory::pystr;

use crate::calls::{self, learning_call, MakeRuntime, Spans};
use crate::items::{Item, Memory, KINDS, MAC_PROFILE_KIND};
use crate::journal::Entry;
use crate::py;
use crate::sql;
use crate::{Error, Result};

pub const SEEN_PREFIX: &str = "Here's what I saw on your Mac: ";
pub const SEEN_SUFFIX: &str = " I saved this as what I observed; tell me if any of it is wrong.";

/// mac_profile.py:20 `MAC_SCHEMA`.
pub fn mac_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["summary", "facts"],
        "properties": {
            "summary": {"type": "string"},
            "facts": {"type": "array", "items": {
                "type": "object", "additionalProperties": false, "required": ["kind", "key", "content"],
                "properties": {"kind": {"type": "string", "enum": KINDS}, "key": {"type": "string"},
                               "content": {"type": "string"}}}}}})
}

fn text_of(v: Option<&Value>, default: &str) -> String {
    v.map_or_else(|| default.to_owned(), py::py_str)
}

/// mac_profile.py:31 `interpret`: one model call reads the gathered facts.
pub async fn interpret(
    make_runtime: &MakeRuntime<'_>,
    prompts: &PromptRoot,
    facts: &Value,
    device_id: &str,
    spans: Option<&dyn Spans>,
    usage: &dyn Fn(&str, &Value),
) -> Result<Value> {
    let system = prompts
        .load("memory/mac_profile.md", &[])
        .map_err(|e| Error::Crash("OSError", e.to_string()))?;
    let task = calls::learning_task(
        &format!("mac-profile-{}", text_of(facts.get("scanned_at"), "")),
        "what the owner does on this Mac",
        device_id,
        "mac-profile",
    );
    let schema = mac_schema();
    let made = make_runtime("worker", "supervisor", &[], &schema)?;
    let ctx = ferb_py::json::dumps_with(
        facts,
        ferb_py::json::Style {
            ascii: false,
            sort_keys: false,
            indent: Some(1),
        },
    );
    let text = learning_call(
        &made,
        &task,
        &system,
        &ctx,
        "mac profile facts",
        &schema,
        "mac_profile",
        spans,
        usage,
    )
    .await?;
    let mut answer = calls::loads(&text)?;
    if !answer.is_object() {
        return Err(Error::Value(
            "the Mac profile answer is not an object".into(),
        ));
    }
    answer["made_by"] = json!(format!(
        "model:{}|{}",
        made.rt.name(),
        calls::prompt_tag("memory/mac_profile.md", &system)
    ));
    Ok(answer)
}

/// mac_profile.py:52 `previous`: the current observed items that rest on the gathered Mac facts.
pub fn previous(memory: &Memory) -> Result<Vec<Item>> {
    let rows = memory.store.query(
        "SELECT DISTINCT s.item_id FROM memory_sources s JOIN journal j ON j.id = s.source_id \
         WHERE s.kind = 'journal' AND j.kind = ?",
        sql![MAC_PROFILE_KIND],
    )?;
    let current = memory.current_ids(Some("observed"))?;
    let mut out = Vec::new();
    for r in rows {
        if let Some(it) = memory.get(&r.s("item_id"))? {
            if current.contains(&it.id) {
                out.push(it);
            }
        }
    }
    Ok(out)
}

/// mac_profile.py:63 `store`: the conclusions saved as observed items about the owner; an earlier one whose key isn't
/// given again ends.
pub fn store(
    memory: &Memory,
    append: &dyn Fn(&str, Value) -> Result<Entry>,
    facts: &Value,
    answer: &Value,
) -> Result<Value> {
    let entry = append(
        MAC_PROFILE_KIND,
        json!({"facts": facts, "made_by": text_of(answer.get("made_by"), "")}),
    )?;
    let old = previous(memory)?;
    let (mut saved, mut refused, mut keys) = (Vec::new(), Vec::new(), BTreeSet::new());
    let list = answer
        .get("facts")
        .filter(|v| ferb_py::value::truthy(v))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for f in &list {
        let got = memory.save(
            &text_of(f.get("kind"), "fact"),
            "owner",
            &text_of(f.get("key"), ""),
            &text_of(f.get("content"), ""),
            "observed",
            &[("journal".into(), entry.id.clone())],
            0.6,
            &[],
            "what FERB saw on your Mac",
            None,
        );
        match got {
            Ok(s) => {
                let it = s.item;
                keys.insert((it.kind.clone(), it.key.clone()));
                saved.push(json!({"id": it.id, "kind": it.kind, "key": it.key,
                                  "text": pystr::split_ws(&it.content).collect::<Vec<_>>().join(" ")}));
            }
            Err(e @ (Error::MemoryRefused(_) | Error::Value(_))) => refused.push(e.to_string()),
            Err(e) => return Err(e),
        }
    }
    let saved_ids: BTreeSet<String> = saved.iter().map(|s| py::py_str(&s["id"])).collect();
    for it in old {
        if !keys.contains(&(it.kind.clone(), it.key.clone())) && !saved_ids.contains(&it.id) {
            memory.retire(&it.id, "the Mac's facts changed")?;
        }
    }
    let summary = pystr::split_ws(&text_of(answer.get("summary"), ""))
        .collect::<Vec<_>>()
        .join(" ");
    let shown = if summary.is_empty() {
        String::new()
    } else {
        format!("{SEEN_PREFIX}{summary}{SEEN_SUFFIX}")
    };
    Ok(
        json!({"summary": summary, "shown": shown, "facts": saved, "refused": refused, "journal_id": entry.id}),
    )
}
