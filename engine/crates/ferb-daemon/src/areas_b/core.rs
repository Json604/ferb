//! What every group-B handler reads from the daemon (ferb/daemon/main.py's own state): FERB's home, config.toml, the
//! redactor, the journal, the event bus and the clock. The composition (main.py's port) builds one [`Host`] from the
//! real stores; tests build it from a temporary home, the real `Redactor` and recording journal and bus.

use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::config::Config;
use ferb_core::interfaces::Clock;
use ferb_core::when::DateTime;
use ferb_rpc::{HandlerError, HandlerResult};
use ferb_sanitize::redact::Redactor;
use serde_json::{Map, Value};

/// The journal (ferb/memory/journal.py `Journal.append`): it redacts `data` and works out the origin when `origin`
/// is None. A failed write is the caller's error (never dropped).
pub trait Journal: Send + Sync {
    fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> Result<(), String>;
}

/// The daemon's published events (compose.py `_publish` → `events.Events.publish`): redacted once, then handed to
/// the consumers.
pub trait Bus: Send + Sync {
    fn publish(&self, topic: &str, data: Value);
}

/// The daemon's shared state as group B's handlers use it.
#[derive(Clone)]
pub struct Host {
    pub home: PathBuf,
    pub config: Arc<Config>,
    pub redactor: Arc<Redactor>,
    pub journal: Arc<dyn Journal>,
    pub bus: Arc<dyn Bus>,
    pub clock: Arc<dyn Clock>,
}

impl Host {
    /// `self.config.get(section, key, default)`: a config FERB refuses to read (others can write it) is FERB's own
    /// error, as Python's InsecureConfig is.
    pub fn get(&self, section: &str, key: &str) -> Result<Option<Value>, HandlerError> {
        self.config
            .get(section, key)
            .map_err(|e| refusal(e.to_string()))
    }

    /// `config.get(section, key, default)` with the default applied when the key is absent.
    pub fn get_or(&self, section: &str, key: &str, default: Value) -> Result<Value, HandlerError> {
        Ok(self.get(section, key)?.unwrap_or(default))
    }

    pub fn set(&self, section: &str, key: &str, value: &Value) -> Result<(), HandlerError> {
        self.config
            .set(section, key, value)
            .map_err(|e| refusal(e.to_string()))
    }

    pub fn remove(&self, section: &str, key: &str) -> Result<(), HandlerError> {
        self.config
            .remove(section, &[key])
            .map_err(|e| refusal(e.to_string()))
    }

    /// `self.redact(value)`.
    pub fn redact(&self, value: &Value) -> Value {
        self.redactor.value(value)
    }

    /// `self._redact_text(text)` (main.py:655).
    pub fn redact_text(&self, text: &str) -> String {
        self.redactor.text(text)
    }

    /// `self._journal().append(kind, data, origin=…)`.
    pub fn journal(
        &self,
        kind: &str,
        data: Value,
        origin: Option<&str>,
    ) -> Result<(), HandlerError> {
        self.journal
            .append(kind, data, origin)
            .map_err(|e| HandlerError::fault("DatabaseError", e))
    }

    /// `self._publish(topic, data)`.
    pub fn publish(&self, topic: &str, data: Value) {
        self.bus.publish(topic, data)
    }

    pub fn now(&self) -> DateTime {
        self.clock.now()
    }

    /// `self.clock.now().isoformat()`.
    pub fn now_iso(&self) -> String {
        ferb_core::when::isoformat(&self.clock.now())
    }
}

/// A `FerbError` with these words: a unary call answers `-32004` with them, a stream `"FerbError: <words>"`.
pub fn refusal(message: impl Into<String>) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

/// The params of a handler, read as Python reads `params.get(...)`.
pub struct P<'a>(pub &'a Map<String, Value>);

impl<'a> P<'a> {
    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// `str(params.get(key) or "")`.
    pub fn text_or_empty(&self, key: &str) -> String {
        match self.0.get(key) {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        }
    }

    /// `str(params.get(key, default))`: a present `null` is Python's "None".
    pub fn text(&self, key: &str, default: &str) -> String {
        match self.0.get(key) {
            Some(v) => ferb_py::value::py_str(v),
            None => default.to_owned(),
        }
    }

    /// `params.get(key) is True`.
    pub fn is_true(&self, key: &str) -> bool {
        matches!(self.0.get(key), Some(Value::Bool(true)))
    }

    /// `bool(params.get(key))`.
    pub fn truthy(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(ferb_py::value::truthy)
    }
}

/// A JSON object from a `json!` value, as a handler's result.
pub fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".to_owned(), other);
            map
        }
    }
}

/// `Ok(object(value))`.
pub fn ok(value: Value) -> HandlerResult {
    Ok(object(value))
}
