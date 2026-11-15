//! The typed answer's form per provider: a seam to ferb/runtimes/schema_shape.py's reshaping (ported in
//! `ferb-context`) and core/structured.py's `check_final`, plus the small pieces of schema_shape.py the adapters
//! use directly: `same`, `Refusals` (RUN-427), `stated`, `repair_note`, `unfenced`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::pyjson::compact;

/// schema_shape.Shape and SchemaTooComplex: ferb-context's, the verified port of schema_shape.py (one type for every
/// crate; these paths are kept for the adapters' callers).
pub use ferb_context::shape::{SchemaTooComplex, Shape};

/// ferb-context's reshaping (`for_anthropic`, `for_gemini`) and `structured.check_final` behind [`Forms`]: what a
/// daemon gives the adapters.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextForms;

impl Forms for ContextForms {
    fn anthropic(&self, schema: &Value) -> Result<Shape, SchemaTooComplex> {
        ferb_context::shape::for_anthropic(schema)
    }
    fn gemini(&self, schema: &Value) -> Result<Shape, SchemaTooComplex> {
        ferb_context::shape::for_gemini(schema)
    }
    fn check_final(&self, text: &str, schema: &Value) -> Result<(), String> {
        ferb_context::structured::check_final(text, Some(schema))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// What the adapters need from the schema port and the structured-answer check.
pub trait Forms: Send + Sync {
    /// schema_shape.for_anthropic (RUN-408).
    fn anthropic(&self, schema: &Value) -> Result<Shape, SchemaTooComplex>;
    /// schema_shape.for_gemini.
    fn gemini(&self, schema: &Value) -> Result<Shape, SchemaTooComplex>;
    /// core/structured.check_final: `Err` holds the ValueError's words.
    fn check_final(&self, text: &str, schema: &Value) -> Result<(), String>;
}

/// How long a refused typed format is remembered (schema_shape.STATED_FOR_S).
pub const STATED_FOR_S: f64 = 24.0 * 3600.0;

/// schema_shape.Refusals: which typed formats a provider refused, each for STATED_FOR_S on an injected monotonic
/// clock; in memory only.
pub struct Refusals {
    monotonic: Box<dyn Fn() -> f64 + Send + Sync>,
    at: Mutex<HashMap<String, f64>>,
}

impl Refusals {
    pub fn new(monotonic: Box<dyn Fn() -> f64 + Send + Sync>) -> Refusals {
        Refusals {
            monotonic,
            at: Mutex::new(HashMap::new()),
        }
    }

    /// The key of (provider, model, wire). Python hashes `json.dumps([...], sort_keys=True)`; the key never leaves
    /// memory, so the sorted text itself serves.
    pub fn key(provider: &str, model: &str, wire: &Value) -> String {
        ferb_py::json::dumps_with(
            &serde_json::json!([provider, model, wire]),
            ferb_py::json::Style {
                ascii: true,
                sort_keys: true,
                indent: None,
            },
        )
    }

    pub fn refused(&self, key: &str) -> bool {
        let now = (self.monotonic)();
        let Ok(mut at) = self.at.lock() else {
            return false;
        };
        match at.get(key).copied() {
            Some(t) if now - t >= STATED_FOR_S => {
                at.remove(key);
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn remember(&self, key: &str) {
        let now = (self.monotonic)();
        if let Ok(mut at) = self.at.lock() {
            at.insert(key.to_string(), now);
        }
    }
}

/// schema_shape.stated: the answer form as instructions.
pub fn stated(schema: &Value) -> String {
    format!(
        "FERB's answer format (required): your final answer is one JSON object that matches this JSON Schema, \
         with nothing before or after it and no code fence. Include every property; use null where the schema \
         allows it for a value that doesn't apply.\n{}",
        compact(schema)
    )
}

/// schema_shape.repair_note.
pub fn repair_note(problem: &str) -> String {
    format!(
        "That answer doesn't fit FERB's answer format ({problem}). Reply again with only the corrected JSON object."
    )
}

/// schema_shape.unfenced: the text inside one Markdown code fence wrapping the whole answer; other text as is.
pub fn unfenced(text: &str) -> String {
    let inner = ferb_py::value::strip(text);
    if !(inner.starts_with("```") && inner.ends_with("```") && ferb_py::value::char_len(inner) >= 6)
    {
        return text.to_string();
    }
    let middle = &inner[3..inner.len() - 3];
    let (first, rest) = match middle.split_once('\n') {
        Some((a, b)) => (a, b),
        None => (middle, ""),
    };
    if matches!(ferb_py::value::strip(first), "" | "json" | "JSON") {
        ferb_py::value::strip(rest).to_string()
    } else {
        text.to_string()
    }
}
