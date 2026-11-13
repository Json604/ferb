//! Reads the fixtures the Python code wrote (rust/fixtures/core/make.py, ENG-3).
#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;

pub fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/core")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Python's JSON text re-written without spaces, keys in Python's order (serde_json keeps insertion order here).
pub fn compact(python_json: &str) -> String {
    let value: Value =
        serde_json::from_str(python_json).unwrap_or_else(|e| panic!("{python_json}: {e}"));
    serde_json::to_string(&value).unwrap()
}

pub fn strs(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}
