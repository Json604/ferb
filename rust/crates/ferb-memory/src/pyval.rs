//! Python's handling of decoded JSON where the readers lean on it: `x.get(k)`, `x or {}`, `for y in x or []`,
//! truthiness, `str(x)`, `float(x)`, `int(x)`. Where Python would raise on a shape the code did not expect (a
//! `.get` on a list, iterating a number), these return [`Crash`]: the import stops there as Python's does.

use serde_json::{Map, Value};

use crate::pystr;

/// An exception Python does not catch at that point (its type name): the caller's import fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crash(pub &'static str);

impl std::fmt::Display for Crash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} on an unexpected shape", self.0)
    }
}

impl std::error::Error for Crash {}

static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();

fn empty() -> &'static Map<String, Value> {
    EMPTY.get_or_init(Map::new)
}

/// Python's truth value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `v or {}` where the result is then read with `.get`: a dict, or empty when `v` is falsy; anything else
/// would raise `AttributeError` at the `.get`.
pub fn obj_or_empty(v: Option<&Value>) -> Result<&Map<String, Value>, Crash> {
    match v {
        None => Ok(empty()),
        Some(Value::Object(m)) => Ok(m),
        Some(x) if !truthy(x) => Ok(empty()),
        Some(_) => Err(Crash("AttributeError")),
    }
}

/// `v.get(key)` on a value that must be a dict.
pub fn get<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, Crash> {
    match v {
        Value::Object(m) => Ok(m.get(key)),
        _ => Err(Crash("AttributeError")),
    }
}

/// The items of `for x in (v or [])`: a list's items, a string's characters, a dict's keys; a truthy number or
/// `True` raises `TypeError`.
pub fn iter_or_empty(v: Option<&Value>) -> Result<Vec<Value>, Crash> {
    match v {
        None => Ok(Vec::new()),
        Some(x) if !truthy(x) => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::String(s)) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Some(Value::Object(m)) => Ok(m.keys().map(|k| Value::String(k.clone())).collect()),
        Some(_) => Err(Crash("TypeError")),
    }
}

/// `str(v)`.
pub fn to_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

/// `str(v or "")`.
pub fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(x) if truthy(x) => to_str(x),
        _ => String::new(),
    }
}

/// `repr(v)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(n) => ferb_py::json::number(n),
        Value::String(s) => pystr::repr(s),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(repr).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(m) => {
            let items: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{}: {}", pystr::repr(k), repr(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// `float(text)` as Python 3.12 reads a string (`PyFloat_FromString`): Unicode digits and spaces, one `_` between
/// digits, `inf`/`infinity`/`nan` in any case.
pub fn float_from_str(text: &str) -> Option<f64> {
    let mut ascii = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else if pystr::is_space(c) {
            ascii.push(' ');
        } else {
            ascii.push(char::from(b'0' + pystr::decimal(c)? as u8));
        }
    }
    let s = ascii.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b');
    let s = if s.contains('_') {
        let mut out = String::with_capacity(s.len());
        let mut prev = '\0';
        for c in s.chars() {
            if c == '_' {
                if !prev.is_ascii_digit() {
                    return None;
                }
            } else {
                if prev == '_' && !c.is_ascii_digit() {
                    return None;
                }
                out.push(c);
            }
            prev = c;
        }
        if prev == '_' {
            return None;
        }
        out
    } else {
        s.to_owned()
    };
    let body = s.strip_prefix(['+', '-']).unwrap_or(&s);
    let lower = body.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        let v = if lower == "nan" {
            f64::NAN
        } else {
            f64::INFINITY
        };
        return Some(if s.starts_with('-') { -v } else { v });
    }
    // digits [. digits] [e [sign] digits], at least one digit before the exponent
    let b = body.as_bytes();
    let mut i = 0;
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    s.parse::<f64>().ok()
}

/// `float(v)`: `None` where Python raises `TypeError` or `ValueError`.
pub fn float(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => float_from_str(s),
        _ => None,
    }
}

/// `int(v)`, saturated to `i64` (the callers clamp it anyway); `Crash` where Python raises.
pub fn int(v: &Value) -> Result<i64, Crash> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.is_u64() {
                Ok(i64::MAX)
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.is_nan() {
                    Err(Crash("ValueError"))
                } else if f.is_infinite() {
                    Err(Crash("OverflowError"))
                } else {
                    Ok(f.trunc() as i64)
                }
            }
        }
        Value::String(s) => int_from_str(s).ok_or(Crash("ValueError")),
        _ => Err(Crash("TypeError")),
    }
}

/// `int(text)` in base 10: Unicode digits and spaces, a sign, one `_` between digits.
fn int_from_str(text: &str) -> Option<i64> {
    let s = pystr::strip(text);
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let mut value: i64 = 0;
    let mut prev_digit = false;
    let mut any = false;
    let mut last_underscore = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            last_underscore = true;
            continue;
        }
        let d = pystr::decimal(c)?;
        value = value.saturating_mul(10).saturating_add(i64::from(d));
        prev_digit = true;
        any = true;
        last_underscore = false;
    }
    if !any || last_underscore {
        return None;
    }
    Some(if neg { -value } else { value })
}
