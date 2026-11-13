//! FERB's provider-neutral answer schemas reshaped into what one provider accepts, and its answer mapped back
//! (ferb/runtimes/schema_shape.py, decisions RUN-408, RUN-427, RUN-447). The core schema stays the one every answer
//! is checked against (`structured::check_final`), after the adapter maps the answer back.
//!
//! - Codex and the OpenAI API take the core schema as it is.
//! - Anthropic (the API's `output_config.format` and Claude Code's typed output): at most 16 union-typed and 24
//!   optional parameters. `for_anthropic` turns each nullable object into a list of at most one item, each nested
//!   nullable scalar into an optional non-null property while the optional budget lasts, and refuses
//!   (`SchemaTooComplex`) a schema still over a limit.
//! - Gemini: an enum holding null is written as `anyOf [the enum, null]`.
//! - The fallback (RUN-427): the form stated in the instructions (`stated`), a misfit told back once
//!   (`repair_note`), and `Refusals` remembering for `STATED_FOR_S` which typed formats a provider refused.

use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::pyvalue::{dumps, loads, Dict, Dumps, List, Py};

/// Anthropic's documented limits (structured-outputs, "Schema complexity limits", 2026-10-30).
pub const ANTHROPIC_MAX_UNIONS: usize = 16;
pub const ANTHROPIC_MAX_OPTIONAL: usize = 24;
pub const WRAPPED_NOTE: &str =
    "A list standing for one optional object: empty for none, otherwise exactly one item.";
/// The JSON Schema keywords Gemini's structured output documents, plus `$ref`, `$defs`, `description`, `title`.
pub const GEMINI_KEYWORDS: [&str; 17] = [
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "prefixItems",
    "minItems",
    "maxItems",
    "enum",
    "format",
    "minimum",
    "maximum",
    "anyOf",
    "$ref",
    "$defs",
    "description",
    "title",
];
/// How long a refused typed format is remembered (24 h, a technical retry interval).
pub const STATED_FOR_S: f64 = 24.0 * 3600.0;

/// A core schema still over a provider's documented limit after reshaping: refused before anything is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaTooComplex(pub String);

impl std::fmt::Display for SchemaTooComplex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemaTooComplex {}

/// A way back from a provider's answer to the core form, as ferb-runtimes' former `Shape` carried it.
pub type Back = std::sync::Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// A core answer schema and the form one provider receives (`wire`); `mapped` when answers need mapping back. The one
/// `Shape` type: ferb-runtimes' adapters use it (`ferb_runtimes::forms::Shape` is this).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub core: Value,
    pub wire: Value,
    pub mapped: bool,
}

impl Shape {
    /// `same`: the core schema as it is (Codex, the OpenAI API).
    pub fn same(schema: &Value) -> Shape {
        Shape {
            core: schema.clone(),
            wire: schema.clone(),
            mapped: false,
        }
    }

    /// A reshaped form (`mapped`): its way back is always the core schema's own (`back(core, answer)`), so `_back`
    /// is not called. Kept so code written for ferb-runtimes' former `Shape` (which carried the way back as a
    /// function) compiles unchanged; build shapes with [`for_anthropic`] / [`for_gemini`] instead.
    pub fn mapped(core: Value, wire: Value, _back: Back) -> Shape {
        Shape {
            core,
            wire,
            mapped: true,
        }
    }

    /// [`Shape::value`] on a decoded JSON answer (what the runtimes' adapters hold).
    pub fn value_json(&self, answer: &Value) -> Value {
        if self.mapped {
            self.value(Py::from_value(answer)).to_value()
        } else {
            answer.clone()
        }
    }

    /// The provider's answer in the core shape.
    pub fn value(&self, answer: Py) -> Py {
        if self.mapped {
            back(&self.core, answer)
        } else {
            answer
        }
    }

    /// One top-level property's answer in the core shape (the planner's streamed reply reads them one by one).
    pub fn field(&self, key: &str, value: Py) -> Py {
        match self.core.get("properties").and_then(|p| p.get(key)) {
            Some(prop @ Value::Object(_)) if self.mapped => back(prop, value),
            _ => value,
        }
    }

    /// The final answer's text in the core shape (`json.dumps(…, ensure_ascii=False)`); text that isn't JSON is
    /// left for FERB's check to name.
    pub fn answer_text(&self, text: &str) -> String {
        if !self.mapped {
            return text.to_string();
        }
        match loads(text) {
            Ok(answer) => dumps(&back(&self.core, answer), Dumps::UTF8),
            Err(_) => text.to_string(),
        }
    }
}

// ---------- reading a schema ----------

fn branches(node: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    node.get("anyOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|b| b.get("type") != Some(&json!("null")))
        .collect()
}

/// schema_shape.py:90: a type list holding "null", an `anyOf` branch of type null, or an enum holding null.
pub fn nullable(node: &Value) -> bool {
    let Value::Object(node) = node else {
        return false;
    };
    let typed =
        matches!(node.get("type"), Some(Value::Array(kinds)) if kinds.contains(&json!("null")));
    let branch = node
        .get("anyOf")
        .and_then(Value::as_array)
        .is_some_and(|bs| bs.iter().any(|b| b.get("type") == Some(&json!("null"))));
    let in_enum =
        matches!(node.get("enum"), Some(Value::Array(values)) if values.contains(&Value::Null));
    typed || branch || in_enum
}

/// schema_shape.py:99: `anyOf`, or a type list.
pub fn is_union(node: &Value) -> bool {
    match node {
        Value::Object(map) => {
            map.contains_key("anyOf") || matches!(map.get("type"), Some(Value::Array(_)))
        }
        _ => false,
    }
}

/// schema_shape.py:103 `_non_null`: the one non-null form of a nullable property; None when it has several.
fn non_null(node: &Map<String, Value>) -> Option<Value> {
    if node.contains_key("anyOf") {
        let rest = branches(node);
        if rest.len() != 1 {
            return None;
        }
        let mut out = rest[0].clone();
        if let Some(description) = node.get("description") {
            if !out.contains_key("description") {
                out.insert("description".into(), description.clone());
            }
        }
        return Some(Value::Object(out));
    }
    if let Some(Value::Array(kinds)) = node.get("type") {
        let kinds: Vec<&Value> = kinds.iter().filter(|k| *k != &json!("null")).collect();
        if kinds.len() != 1 {
            return None;
        }
        // every key but "type" in order, then "type" last (a dict comprehension, then an assignment)
        let mut out: Map<String, Value> = node
            .iter()
            .filter(|(k, _)| *k != "type")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.insert("type".into(), kinds[0].clone());
        if let Some(Value::Array(values)) = out.get("enum") {
            let kept: Vec<Value> = values.iter().filter(|v| !v.is_null()).cloned().collect();
            out.insert("enum".into(), Value::Array(kept));
        }
        return Some(Value::Object(out));
    }
    None
}

fn children(node: &Map<String, Value>) -> Vec<&Value> {
    let mut out: Vec<&Value> = node
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::values)
        .collect();
    if let Some(items @ Value::Object(_)) = node.get("items") {
        out.push(items);
    }
    for key in ["anyOf", "allOf"] {
        out.extend(
            node.get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        );
    }
    out.extend(
        node.get("$defs")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(Map::values),
    );
    out
}

/// schema_shape.py:134: (parameters with union types, optional parameters), every property at any depth.
pub fn complexity(schema: &Value) -> (usize, usize) {
    let (mut unions, mut optional) = (0, 0);
    let mut stack = vec![schema];
    while let Some(node) = stack.pop() {
        let Value::Object(node) = node else { continue };
        let required: Vec<&Value> = node
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().collect())
            .unwrap_or_default();
        for (name, prop) in node
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            unions += usize::from(is_union(prop));
            optional += usize::from(!required.contains(&&json!(name)));
        }
        stack.extend(children(node));
    }
    (unions, optional)
}

/// schema_shape.py:150: every schema keyword used (property names excluded), sorted.
pub fn keywords(schema: &Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![schema];
    while let Some(node) = stack.pop() {
        if let Value::Object(map) = node {
            found.extend(map.keys().cloned());
            stack.extend(children(map));
        }
    }
    found
}

/// schema_shape.py:162: an enum holding null anywhere.
pub fn enum_with_null(schema: &Value) -> bool {
    let mut stack = vec![schema];
    while let Some(node) = stack.pop() {
        if let Value::Object(map) = node {
            if matches!(map.get("enum"), Some(Value::Array(values)) if values.contains(&Value::Null))
            {
                return true;
            }
            stack.extend(children(map));
        }
    }
    false
}

// ---------- Anthropic ----------

/// schema_shape.py:175 `for_anthropic`: the schema within Anthropic's union and optional limits.
pub fn for_anthropic(schema: &Value) -> Result<Shape, SchemaTooComplex> {
    let (_, optional) = complexity(schema);
    let mut budget = ANTHROPIC_MAX_OPTIONAL as i64 - optional as i64;
    let wire = anthropic_walk(schema, true, &mut budget);
    let (unions, optional) = complexity(&wire);
    if unions > ANTHROPIC_MAX_UNIONS || optional > ANTHROPIC_MAX_OPTIONAL {
        return Err(SchemaTooComplex(format!(
            "this answer form has {unions} union-typed and {optional} optional parameters after reshaping; \
             Anthropic allows {ANTHROPIC_MAX_UNIONS} and {ANTHROPIC_MAX_OPTIONAL}"
        )));
    }
    let mapped = wire != *schema;
    Ok(Shape {
        core: schema.clone(),
        wire,
        mapped,
    })
}

/// A property's wire form, and whether it stops being required.
fn anthropic_prop(node: &Value, top: bool, budget: &mut i64) -> (Value, bool) {
    let Value::Object(map) = node else {
        return (node.clone(), false);
    };
    let inner = if nullable(node) { non_null(map) } else { None };
    if let Some(inner) = &inner {
        if inner.get("type") == Some(&json!("object")) {
            let note = [map.get("description"), Some(&json!(WRAPPED_NOTE))]
                .into_iter()
                .flatten()
                .filter(|v| ferb_py::value::truthy(v))
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            let items = anthropic_walk(inner, false, budget);
            return (
                json!({"type": "array", "items": items, "description": note}),
                false,
            );
        }
        if !top && *budget > 0 {
            *budget -= 1;
            return (anthropic_walk(inner, false, budget), true);
        }
    }
    (anthropic_walk(node, false, budget), false)
}

fn anthropic_walk(node: &Value, top: bool, budget: &mut i64) -> Value {
    let Value::Object(map) = node else {
        return node.clone();
    };
    let mut out: Map<String, Value> = map
        .iter()
        .filter(|(k, _)| {
            !["properties", "required", "items", "anyOf", "$defs"].contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(props_in) = map.get("properties") {
        let mut required: Vec<Value> = map
            .get("required")
            .filter(|r| ferb_py::value::truthy(r))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut props = Map::new();
        for (name, sub) in props_in.as_object().into_iter().flatten() {
            let (wire, dropped) = anthropic_prop(sub, top, budget);
            props.insert(name.clone(), wire);
            if dropped {
                if let Some(at) = required.iter().position(|r| r == &json!(name)) {
                    required.remove(at);
                }
            }
        }
        out.insert("properties".into(), Value::Object(props));
        out.insert("required".into(), Value::Array(required));
    } else if let Some(required) = map.get("required") {
        out.insert("required".into(), required.clone());
    }
    if let Some(items) = map.get("items") {
        out.insert("items".into(), anthropic_walk(items, false, budget));
    }
    if let Some(any_of) = map.get("anyOf") {
        let walked = any_of
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| anthropic_walk(b, false, budget))
            .collect();
        out.insert("anyOf".into(), Value::Array(walked));
    }
    if let Some(defs) = map.get("$defs") {
        let walked = defs
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), anthropic_walk(v, false, budget)))
            .collect();
        out.insert("$defs".into(), Value::Object(walked));
    }
    Value::Object(out)
}

// ---------- Gemini ----------

/// schema_shape.py:225 `for_gemini`: an enum holding null becomes `anyOf [the enum without null, null]`.
pub fn for_gemini(schema: &Value) -> Result<Shape, SchemaTooComplex> {
    Ok(Shape {
        core: schema.clone(),
        wire: gemini_walk(schema)?,
        mapped: false,
    })
}

fn gemini_walk(node: &Value) -> Result<Value, SchemaTooComplex> {
    let map = match node {
        Value::Array(items) => {
            return items
                .iter()
                .map(gemini_walk)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Value::Object(map) => map,
        other => return Ok(other.clone()),
    };
    let mut out = map.clone();
    if let Some(Value::Object(props)) = map.get("properties") {
        let mut walked = Map::new();
        for (k, v) in props {
            walked.insert(k.clone(), gemini_walk(v)?);
        }
        out.insert("properties".into(), Value::Object(walked));
    }
    if let Some(Value::Object(defs)) = map.get("$defs") {
        let mut walked = Map::new();
        for (k, v) in defs {
            walked.insert(k.clone(), gemini_walk(v)?);
        }
        out.insert("$defs".into(), Value::Object(walked));
    }
    for key in ["items", "anyOf", "allOf"] {
        if let Some(v) = map.get(key) {
            out.insert(key.into(), gemini_walk(v)?);
        }
    }
    if let Some(Value::Array(values)) = out.get("enum").cloned() {
        if values.contains(&Value::Null) {
            let declared = out.get("type").cloned().unwrap_or(Value::Null);
            let all = match &declared {
                Value::Array(kinds) => kinds.clone(),
                other => vec![other.clone()],
            };
            let kinds: Vec<Value> = all
                .into_iter()
                .filter(|k| ferb_py::value::truthy(k) && k != &json!("null"))
                .collect();
            if kinds.len() != 1 {
                return Err(SchemaTooComplex(
                    "an enum holding null needs one other type for Gemini".into(),
                ));
            }
            let mut branch = Map::new();
            branch.insert("type".into(), kinds[0].clone());
            branch.insert(
                "enum".into(),
                Value::Array(values.into_iter().filter(|v| !v.is_null()).collect()),
            );
            for (k, v) in &out {
                if !["type", "enum", "description"].contains(&k.as_str()) {
                    branch.insert(k.clone(), v.clone());
                }
            }
            let mut wrapped = Map::new();
            wrapped.insert(
                "anyOf".into(),
                json!([Value::Object(branch), {"type": "null"}]),
            );
            if let Some(description) = out.get("description") {
                wrapped.insert("description".into(), description.clone());
            }
            out = wrapped;
        }
    }
    Ok(Value::Object(out))
}

// ---------- the answer form stated in the instructions (the fallback, RUN-427) ----------

/// Which typed formats a provider refused (runtime family, model, wire schema), each remembered for
/// `STATED_FOR_S` on an injected monotonic clock. In memory only: a start tries the strict form again.
pub struct Refusals {
    monotonic: Box<dyn Fn() -> f64 + Send + Sync>,
    at: HashMap<String, f64>,
}

impl Refusals {
    pub fn new(monotonic: impl Fn() -> f64 + Send + Sync + 'static) -> Refusals {
        Refusals {
            monotonic: Box::new(monotonic),
            at: HashMap::new(),
        }
    }

    /// SHA-256 of `json.dumps([provider, model or "", wire], sort_keys=True, separators=(",", ":"))`.
    pub fn key(provider: &str, model: Option<&str>, wire: &Value) -> String {
        let list = Py::list(vec![
            Py::str(provider),
            Py::str(model.unwrap_or("")),
            Py::from_value(wire),
        ]);
        let text = dumps(
            &list,
            Dumps {
                ascii: true,
                compact: true,
                sort_keys: true,
            },
        );
        hex::encode(Sha256::digest(text.as_bytes()))
    }

    pub fn refused(&mut self, key: &str) -> bool {
        match self.at.get(key) {
            Some(at) if (self.monotonic)() - at >= STATED_FOR_S => {
                self.at.remove(key);
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn remember(&mut self, key: &str) {
        let now = (self.monotonic)();
        self.at.insert(key.to_string(), now);
    }
}

/// The opening of `stated`'s text.
pub const STATED_PREFIX: &str = "FERB's answer format (required): your final answer is one JSON object that matches \
    this JSON Schema, with nothing before or after it and no code fence. Include every property; use null where the \
    schema allows it for a value that doesn't apply.\n";

/// schema_shape.py:289 `stated`: the answer form as instructions: the core schema, compact.
pub fn stated(schema: &Value) -> String {
    format!(
        "{STATED_PREFIX}{}",
        crate::pyvalue::dumps_value(schema, Dumps::COMPACT_UTF8)
    )
}

/// schema_shape.py:298 `repair_note`.
pub fn repair_note(problem: &str) -> String {
    format!(
        "That answer doesn't fit FERB's answer format ({problem}). Reply again with only the corrected JSON object."
    )
}

/// schema_shape.py:303 `unfenced`: the text inside one Markdown code fence wrapping the whole answer; other text
/// as it is.
pub fn unfenced(text: &str) -> String {
    let inner = ferb_py::value::strip(text);
    if !(inner.starts_with("```") && inner.ends_with("```") && inner.chars().count() >= 6) {
        return text.to_string();
    }
    let body = &inner[3..inner.len() - 3];
    let (first, rest) = body.split_once('\n').unwrap_or((body, ""));
    if ["", "json", "JSON"].contains(&ferb_py::value::strip(first)) {
        ferb_py::value::strip(rest).to_string()
    } else {
        text.to_string()
    }
}

// ---------- answers back to the core shape ----------

/// schema_shape.py:328 `_kinds`: the node's type names, "number" too when it names "integer".
fn kinds(node: &Map<String, Value>) -> Vec<&str> {
    let mut out: Vec<&str> = match node.get("type") {
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(kind)) => vec![kind.as_str()],
        _ => Vec::new(),
    };
    if out.contains(&"integer") {
        out.push("number");
    }
    out
}

/// schema_shape.py:334 `_back`: `value` (a provider's answer at `node` of the core schema) in the core shape.
pub fn back(node: &Value, value: Py) -> Py {
    let Value::Object(map) = node else {
        return value;
    };
    if matches!(value, Py::None) {
        return value;
    }
    let inner = if nullable(node) { non_null(map) } else { None };
    let value = match (inner, value) {
        // a list standing for a nullable object (`for_anthropic`): its one item, null when empty; a longer list
        // is left for FERB's check to name
        (Some(inner), Py::List(mut list)) if inner.get("type") == Some(&json!("object")) => {
            let mut items = std::mem::take(&mut list.0);
            return match items.len() {
                0 => Py::None,
                1 => back(&inner, items.pop().expect("one item")),
                _ => Py::List(List(items)),
            };
        }
        (_, value) => value,
    };
    back_rest(map, value)
}

fn back_rest(map: &Map<String, Value>, value: Py) -> Py {
    if map.contains_key("anyOf") {
        let rest = branches(map);
        let found = rest
            .into_iter()
            .find(|b| kinds(b).contains(&value.type_of()));
        return match found {
            Some(branch) => back(&Value::Object(branch.clone()), value),
            None => value,
        };
    }
    match value {
        Py::Dict(mut dict) if map.contains_key("properties") => {
            let mut given = std::mem::take(&mut dict.0);
            let mut out = indexmap::IndexMap::new();
            for (name, sub) in map["properties"].as_object().into_iter().flatten() {
                if let Some(v) = given.shift_remove(name) {
                    out.insert(name.clone(), back(sub, v));
                } else if nullable(sub) {
                    out.insert(name.clone(), Py::None);
                }
            }
            // left for FERB's check to name
            for (k, v) in given {
                out.entry(k).or_insert(v);
            }
            Py::Dict(Dict(out))
        }
        Py::List(mut list) if matches!(map.get("items"), Some(Value::Object(_))) => {
            let items = &map["items"];
            Py::list(
                std::mem::take(&mut list.0)
                    .into_iter()
                    .map(|v| back(items, v))
                    .collect(),
            )
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn a_refusal_is_remembered_for_a_day_on_the_injected_clock() {
        let now = Arc::new(AtomicU64::new(1000));
        let clock = now.clone();
        let mut refusals = Refusals::new(move || clock.load(Ordering::SeqCst) as f64);
        assert!(!refusals.refused("k"));
        refusals.remember("k");
        now.store(1000 + 86_399, Ordering::SeqCst);
        assert!(refusals.refused("k"));
        now.store(1000 + 86_400, Ordering::SeqCst);
        assert!(!refusals.refused("k"));
        assert!(!refusals.refused("k"), "forgotten once expired");
    }

    #[test]
    fn a_schema_over_the_limits_is_refused_before_sending() {
        // tests/test_schema_shape.py test_a_schema_still_over_a_limit_is_refused_before_sending
        let mut props = Map::new();
        for i in 0..20 {
            props.insert(
                format!("f{i}"),
                json!({"type": "object", "additionalProperties": false, "required": ["v"],
                       "properties": {"v": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}}),
            );
        }
        let required: Vec<String> = props.keys().cloned().collect();
        let schema = json!({"type": "object", "additionalProperties": false, "required": required,
                            "properties": props});
        let err = for_anthropic(&schema).unwrap_err();
        assert!(err.0.contains("Anthropic allows"));
    }
}
