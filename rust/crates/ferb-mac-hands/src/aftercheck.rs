//! The uniform after-action read (ferb/act/aftercheck.py, V2): what an acting tool observed changed and whether a
//! procedure's expect matched. A tool that read nothing back reports changed=None (unknown), never a false pass.

use serde_json::{json, Map, Value};

/// aftercheck.py:8: mac tools nest their facts under `facts`; browser.act spreads them into the result.
pub fn facts_of(content: &Map<String, Value>) -> Map<String, Value> {
    match content.get("facts") {
        Some(Value::Object(m)) => m.clone(),
        _ => content.clone(),
    }
}

fn py_str_or_empty(v: Option<&Value>) -> String {
    v.map(ferb_py::value::py_str).unwrap_or_default()
}

/// aftercheck.py:15.
pub fn after_of(facts: &Map<String, Value>) -> Value {
    let note = py_str_or_empty(facts.get("note"));
    let value = facts.get("value").cloned().unwrap_or(Value::Null);
    if facts.contains_key("value_matches") || facts.get("method") == Some(&json!("ax_value")) {
        let mut matched = facts.get("value_matches").cloned().unwrap_or(Value::Null);
        if matched.is_null() {
            matched = facts.get("accepted").cloned().unwrap_or(Value::Null);
        }
        if matched.is_null() {
            return json!({"read": "none", "changed": null, "value": value, "note": note});
        }
        return json!({"read": "value", "changed": ferb_py::value::truthy(&matched), "value": value, "note": note});
    }
    if let Some(observed_change) = facts.get("observed_change") {
        let page = facts.get("observed").is_some_and(Value::is_string);
        let note = if page && note.is_empty() {
            py_str_or_empty(facts.get("observed"))
        } else {
            note
        };
        return json!({"read": if page { "page" } else { "ax" },
                      "changed": if ferb_py::value::truthy(observed_change) { json!(true) } else { Value::Null },
                      "value": value, "note": note});
    }
    if let Some(Value::Bool(b)) = facts.get("bytes_match") {
        return json!({"read": "value", "changed": b, "value": null, "note": note});
    }
    json!({"read": "none", "changed": null, "value": null, "note": note})
}

/// aftercheck.py:37.
pub fn compare(expect: &Map<String, Value>, after: &Value) -> Value {
    if let Some(want) = expect.get("value_contains") {
        let v = after.get("value").unwrap_or(&Value::Null);
        if v.is_null() {
            return json!({"met": null, "evidence": "no value was read back after the action"});
        }
        let ok = ferb_sanitize::parity::casefold(&ferb_py::value::py_str(v)).contains(
            &ferb_sanitize::parity::casefold(&ferb_py::value::py_str(want)),
        );
        return json!({"met": ok, "evidence": format!("after value {} the expected text",
                                                    if ok { "contains" } else { "does not contain" })});
    }
    if let Some(want) = expect.get("changed") {
        let changed = after.get("changed").unwrap_or(&Value::Null);
        if changed.is_null() {
            return json!({"met": null, "evidence": "nothing was read back after the action"});
        }
        let c = ferb_py::value::truthy(changed);
        let ok = c == ferb_py::value::truthy(want);
        return json!({"met": ok, "evidence": format!("the screen {} as {}",
                                                    if c { "changed" } else { "did not change" },
                                                    if ok { "expected" } else { "not expected" })});
    }
    json!({"met": null, "evidence": "no expectation to compare"})
}
