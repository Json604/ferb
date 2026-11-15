//! What a watcher saw (`ferb/sense/trail.py:38` `Observation`), the redaction seam every capture goes through, and the
//! few Python value rules the observations lean on.

use serde_json::Value;

/// A JSON object in insertion order, as a Python dict.
pub type Dict = serde_json::Map<String, Value>;

/// trail.py:38: what a watcher saw. `hints` help attribution and are never stored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observation {
    pub source: String,
    pub data: Dict,
    pub hints: Dict,
}

impl Observation {
    pub fn new(source: &str, data: Dict) -> Observation {
        Observation {
            source: source.to_string(),
            data,
            hints: Dict::new(),
        }
    }

    pub fn with_hints(source: &str, data: Dict, hints: Dict) -> Observation {
        Observation {
            source: source.to_string(),
            data,
            hints,
        }
    }

    /// `{"actor": actor}` hints, the watchers' usual.
    pub fn by(source: &str, data: Dict, actor: &str) -> Observation {
        let mut hints = Dict::new();
        hints.insert("actor".into(), Value::from(actor));
        Observation::with_hints(source, data, hints)
    }
}

/// The capture redactor (`ferb/sanitize/redact.py` `Redactor`): code redacts at capture (rule 7). A watcher holds one
/// and every text it keeps passes through it before it leaves the watcher.
pub trait Redact: Send + Sync {
    /// `redactor(obj)`: a record redacted, its shape kept.
    fn value(&self, v: &Value) -> Value;
    /// `redactor.text(s)`.
    fn text(&self, s: &str) -> String;
    /// `redactor.field(text, role=…, label=…)`: the capture boundary for a typed field (redact.py:243).
    fn field(&self, text: &str, role: &str, label: &str) -> String;

    /// `redactor(dict)` for an object.
    fn dict(&self, d: &Dict) -> Dict {
        match self.value(&Value::Object(d.clone())) {
            Value::Object(m) => m,
            _ => Dict::new(),
        }
    }
}

impl Redact for ferb_sanitize::Redactor {
    fn value(&self, v: &Value) -> Value {
        ferb_sanitize::Redactor::value(self, v)
    }
    fn text(&self, s: &str) -> String {
        ferb_sanitize::Redactor::text(self, s)
    }
    fn field(&self, text: &str, role: &str, label: &str) -> String {
        ferb_sanitize::Redactor::field(
            self,
            text,
            &ferb_sanitize::Field {
                role,
                label,
                ..Default::default()
            },
        )
    }
}

/// `lambda v: v` (the Python tests' redactor): nothing changed.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRedact;

impl Redact for NoRedact {
    fn value(&self, v: &Value) -> Value {
        v.clone()
    }
    fn text(&self, s: &str) -> String {
        s.to_string()
    }
    fn field(&self, text: &str, _role: &str, _label: &str) -> String {
        text.to_string()
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// `str(d.get(key) or "")` for a value that is text (other truthy values as Python's `str`).
pub fn text_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    }
}

/// `str(value)` (None as "None").
pub fn py_str(v: Option<&Value>) -> String {
    match v {
        Some(v) => ferb_py::value::py_str(v),
        None => "None".to_string(),
    }
}

/// memory/summary.py:27: the journal kinds of a day's summary (code-built) and narrative (model-written).
pub const SUMMARY_KIND: &str = "trail.day_summary";
pub const NARRATIVE_KIND: &str = "trail.day_narrative";

/// `{k: v, ...}` from pairs, in order.
pub fn dict<const N: usize>(pairs: [(&str, Value); N]) -> Dict {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// `json.dumps(value, ensure_ascii=False)`.
pub fn dumps(v: &Value) -> String {
    ferb_py::json::dumps_utf8(v)
}
