//! Python's handling of decoded JSON values and text, where the ports lean on it: `dict.get`, truthiness,
//! `str()`, `==`, `str.split()`, `str.strip()`, slicing by character.

use serde_json::{Map, Value};

use crate::json;

/// `value.get(key)` on a decoded object; `None` for a missing key. Python raises on a value
/// that isn't an object; the stores only read objects FERB wrote, and here anything else reads
/// as having no keys.
pub fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|map| map.get(key))
}

/// Python's truth value of a decoded JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(value)` of a decoded JSON value. Exact for text, numbers, booleans and None; a list or
/// an object is written as its JSON text, which is NOT Python's `repr` (single quotes): no
/// store reads one where FERB writes text.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(n) => json::number(n),
        Value::String(s) => s.clone(),
        other => json::dumps_utf8(other),
    }
}

/// Python's `a == b` for decoded JSON: numbers by value (`1 == 1.0`, `True == 1`), objects
/// whatever their key order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Number(n) => n.as_f64(),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if x.is_i64() && y.is_i64() => x == y,
        (Value::Number(x), Value::Number(y)) if x.is_u64() && y.is_u64() => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| py_eq(p, q)))
        }
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// What `str.split()`, `str.strip()` and `str.isspace()` call whitespace (Python 3.12, Unicode 15.0): Rust's
/// `White_Space` plus the four information separators U+001C..U+001F, which Python counts and Rust doesn't.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `text.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_py_space)
}

/// `" ".join(text.split())`.
pub fn squeeze(text: &str) -> String {
    text.split(is_py_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text[:count]`, counted in characters as Python counts them.
pub fn prefix(text: &str, count: usize) -> &str {
    match text.char_indices().nth(count) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// `len(text)`.
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// `a | b` for two objects: `b`'s values win, a key already in `a` keeps its place.
pub fn merged(a: &Map<String, Value>, b: &Map<String, Value>) -> Map<String, Value> {
    let mut out = a.clone();
    for (key, value) in b {
        out.insert(key.clone(), value.clone());
    }
    out
}
