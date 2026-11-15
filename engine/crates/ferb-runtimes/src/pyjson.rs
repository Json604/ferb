//! The JSON forms the Python adapters' libraries write and read, beside `ferb_py::json`: httpx's request bodies
//! (`json.dumps(..., ensure_ascii=False, separators=(",", ":"))`), `JSONDecoder.raw_decode`, and `int()` of a
//! decoded value.

use serde_json::Value;

use crate::error::Raised;

/// httpx's `json=` body (httpx/_content.py `encode_json`): compact, UTF-8, floats as Python's `repr`.
pub fn compact(value: &Value) -> String {
    let mut out = String::new();
    write_compact(value, &mut out);
    out
}

fn write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&ferb_py::json::number(n)),
        Value::String(s) => write_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write_compact(v, out);
            }
            out.push('}');
        }
    }
}

/// A string as `json.dumps(..., ensure_ascii=False)` writes it.
fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `json.JSONDecoder().raw_decode(text, idx)`: the value starting exactly at byte `idx` and the byte where it ends.
pub fn raw_decode(text: &str, idx: usize) -> Option<(Value, usize)> {
    let rest = text.get(idx..)?;
    let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
    // Python's scanner starts at idx itself: no leading whitespace is skipped.
    if rest.starts_with(|c: char| c.is_whitespace()) || rest.is_empty() {
        return None;
    }
    match stream.next() {
        Some(Ok(v)) => Some((v, idx + stream.byte_offset())),
        _ => None,
    }
}

/// `int(d.get(key) or 0)` (usage.py `_int`).
pub fn py_int(value: Option<&Value>) -> Result<i64, Raised> {
    let Some(v) = value else { return Ok(0) };
    if !ferb_py::value::truthy(v) {
        return Ok(0);
    }
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(u) = n.as_u64() {
                Ok(u as i64)
            } else {
                Ok(n.as_f64().unwrap_or(0.0).trunc() as i64)
            }
        }
        Value::String(s) => {
            let t = ferb_py::value::strip(s).replace('_', "");
            t.parse::<i64>().map_err(|_| {
                Raised::new(
                    "ValueError",
                    format!("invalid literal for int() with base 10: '{s}'"),
                )
            })
        }
        Value::Array(_) | Value::Object(_) | Value::Null => Err(Raised::new(
            "TypeError",
            format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                crate::error::py_type(v)
            ),
        )),
    }
}

/// `dict.get(key)` on a value that must be a dict (`AttributeError` otherwise).
pub fn get<'a>(value: &'a Value, key: &str) -> Result<Option<&'a Value>, Raised> {
    match value {
        Value::Object(map) => Ok(map.get(key)),
        other => Err(Raised::no_get(other)),
    }
}

/// `value.get(key, default)` where the default is `{}`: the value, or an empty object when missing.
pub fn get_or_empty(value: &Value, key: &str) -> Result<Value, Raised> {
    Ok(get(value, key)?
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default())))
}

/// A JSON object from pairs, keeping their order.
#[macro_export]
macro_rules! obj {
    ($($k:expr => $v:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut m = serde_json::Map::new();
        $( m.insert(($k).to_string(), serde_json::Value::from($v)); )*
        serde_json::Value::Object(m)
    }};
}

/// `repr()` of a decoded JSON value, as Python writes it (`'text'`, `None`, `True`, `[1, 'a']`, `{'k': 1}`).
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => ferb_py::json::number(n),
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `repr(text)`: single quotes unless the text holds one and no double quote; non-printable characters escaped.
pub fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
