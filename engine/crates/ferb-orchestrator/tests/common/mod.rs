//! What the replay tests share: the fixtures, the `Policy` seam answered by the policy part's own port, and the
//! /FX placeholder.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use ferb_orchestrator::ports::Policy;
use serde_json::Value;

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/orchestrator")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

/// ferb/policy, through the policy part's port (ferb-policy): what the Python build's confirm.py and urls.py answer.
pub struct TestPolicy;

impl Policy for TestPolicy {
    fn site_key(&self, value: Option<&str>) -> String {
        // confirm.py:1091 `site_key`
        let text = ferb_orchestrator::py::strip(value.unwrap_or("")).to_string();
        let parsed = if text.is_empty() {
            None
        } else {
            ferb_policy::urls::parse(&format!("https://{text}"))
        };
        let host = match parsed.filter(|p| !p.host.is_empty()) {
            Some(p) => p.host,
            None => ferb_orchestrator::py::lower(&text),
        };
        let host = host.trim_end_matches('.').to_string();
        host.strip_prefix("www.").map(String::from).unwrap_or(host)
    }

    fn account_key(&self, value: Option<&str>) -> String {
        ferb_policy::confirm::account_key(value)
    }

    fn url_host(&self, url: &str) -> Option<String> {
        ferb_policy::urls::parse(url).map(|u| u.host)
    }

    fn from_task(&self, text: &str) -> String {
        ferb_policy::confirm::from_task(text)
    }

    fn reads_as_ferbs_check(&self, text: &str) -> bool {
        ferb_policy::confirm::reads_as_ferbs_check(text)
    }
}

/// Every "/FX" in a value made `root` (the fixture's temporary folder here).
pub fn unfx(value: &Value, root: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace("/FX", root)),
        Value::Array(a) => Value::Array(a.iter().map(|v| unfx(v, root)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.replace("/FX", root), unfx(v, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Every occurrence of `root` written "/FX".
pub fn fx(value: &Value, root: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace(root, "/FX")),
        Value::Array(a) => Value::Array(a.iter().map(|v| fx(v, root)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.replace(root, "/FX"), fx(v, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A temporary folder by its real path (macOS: /private/var/...), as Python's `resolve()` writes it.
pub fn real_tempdir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temp folder");
    let real = std::fs::canonicalize(dir.path()).expect("its real path");
    (dir, real)
}

/// JSON equality with Python's floats: an integral float and an int compare by value, as serde reads both.
pub fn same(a: &Value, b: &Value) -> bool {
    ferb_py::value::py_eq(a, b) && kinds_match(a, b)
}

fn kinds_match(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(_), Value::Bool(_))
        | (Value::Null, Value::Null)
        | (Value::String(_), Value::String(_)) => true,
        (Value::Number(_), Value::Number(_)) => true,
        (Value::Array(x), Value::Array(y)) => x.iter().zip(y).all(|(p, q)| kinds_match(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.keys().eq(y.keys())
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| kinds_match(v, w)))
        }
        _ => false,
    }
}
