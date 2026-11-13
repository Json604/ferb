//! Pydantic's lax ("python mode") validation of the extension's messages, for the field types
//! `ext_protocol.py` declares: what `Model.model_validate(json.loads(text))` accepts and makes of
//! each value. Checked against pydantic 2's own answers in `rust/fixtures/browser/protocol.json`.
//!
//! One deliberate limit: Python's `int` has no size, a Rust one has 64 bits. A JSON integer
//! beyond `i64` (or a float of 2^63 or more, which pydantic refuses anyway) is refused here; no
//! extension sends one.

use serde_json::{Map, Value};

use crate::generated::{DECIMAL_ZEROS, DIGITS_NOT_DECIMAL};

/// Why a value was refused. Pydantic's error text never reaches anyone (`parse` returns `None`,
/// the models are dropped), so only the fact is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

pub type Lax<T> = Result<T, Invalid>;

/// `int`: an integer, a bool, an integral float inside `i64`, or text holding one.
pub fn int(v: &Value) -> Lax<i64> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.is_u64() {
                Err(Invalid) // beyond i64: see the module's note
            } else {
                float_int(n.as_f64().ok_or(Invalid)?)
            }
        }
        Value::String(s) => str_int(s),
        _ => Err(Invalid),
    }
}

fn float_int(f: f64) -> Lax<i64> {
    const LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63
    if f.is_finite() && f.fract() == 0.0 && f > -LIMIT && f < LIMIT {
        // SAFETY of the cast: the value is integral and strictly inside i64's range.
        Ok(f as i64)
    } else {
        Err(Invalid)
    }
}

/// pydantic-core's text to int: surrounding Unicode whitespace dropped, an optional sign, ASCII
/// digits with single underscores between them, and an optional fraction of zeros.
fn str_int(s: &str) -> Lax<i64> {
    let s = s.trim();
    let whole = match s.split_once('.') {
        Some((whole, frac)) => {
            if frac.is_empty() || !frac.bytes().all(|b| b == b'0') {
                return Err(Invalid);
            }
            whole
        }
        None => s,
    };
    let (negative, digits) = match whole.as_bytes().first() {
        Some(b'-') => (true, &whole[1..]),
        Some(b'+') => (false, &whole[1..]),
        _ => (false, whole),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return Err(Invalid);
    }
    let mut value: i128 = 0;
    for b in digits.bytes() {
        match b {
            b'_' => {}
            b'0'..=b'9' => {
                value = value * 10 + i128::from(b - b'0');
                if value > i128::from(i64::MAX) + 1 {
                    return Err(Invalid);
                }
            }
            _ => return Err(Invalid),
        }
    }
    let value = if negative { -value } else { value };
    i64::try_from(value).map_err(|_| Invalid)
}

/// `bool`: a bool, 0 or 1 (integer or float), or one of pydantic's words (any case, no spaces).
pub fn boolean(v: &Value) -> Lax<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => {
            let f = n.as_f64().ok_or(Invalid)?;
            if f == 0.0 {
                Ok(false)
            } else if f == 1.0 {
                Ok(true)
            } else {
                Err(Invalid)
            }
        }
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" if s.is_ascii() => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" if s.is_ascii() => Ok(true),
            _ => Err(Invalid),
        },
        _ => Err(Invalid),
    }
}

/// `str`: text only (JSON never gives bytes).
pub fn string(v: &Value) -> Lax<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => Err(Invalid),
    }
}

/// `str` with `max_length` (code points, as Python counts).
pub fn string_max(v: &Value, max: usize) -> Lax<String> {
    let s = string(v)?;
    if s.chars().count() > max {
        Err(Invalid)
    } else {
        Ok(s)
    }
}

/// `dict[str, Any]`.
pub fn dict(v: &Value) -> Lax<Map<String, Value>> {
    match v {
        Value::Object(m) => Ok(m.clone()),
        _ => Err(Invalid),
    }
}

/// `list[T]`.
pub fn list<T>(v: &Value, item: impl Fn(&Value) -> Lax<T>) -> Lax<Vec<T>> {
    match v {
        Value::Array(items) => items.iter().map(item).collect(),
        _ => Err(Invalid),
    }
}

/// One field of a model: absent gives `default`; present goes through `check`.
pub fn field<T>(
    body: &Map<String, Value>,
    name: &str,
    default: impl FnOnce() -> Lax<T>,
    check: impl FnOnce(&Value) -> Lax<T>,
) -> Lax<T> {
    match body.get(name) {
        None => default(),
        Some(v) => check(v),
    }
}

/// A required field.
pub fn required<T>(
    body: &Map<String, Value>,
    name: &str,
    check: impl FnOnce(&Value) -> Lax<T>,
) -> Lax<T> {
    field(body, name, || Err(Invalid), check)
}

/// `T | None = None`: absent or null is `None`.
pub fn optional<T>(
    body: &Map<String, Value>,
    name: &str,
    check: impl FnOnce(&Value) -> Lax<T>,
) -> Lax<Option<T>> {
    match body.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => check(v).map(Some),
    }
}

/// A `Literal[...]` of strings: exactly one of them.
pub fn literal(v: &Value, allowed: &[&str]) -> Lax<String> {
    match v {
        Value::String(s) if allowed.contains(&s.as_str()) => Ok(s.clone()),
        _ => Err(Invalid),
    }
}

/// `str.isdigit()` for one character (Python 3.12's tables): decimal digits plus the other digits
/// (superscripts, circled numbers...), whose `int()` fails.
pub fn is_digit(c: char) -> bool {
    decimal_value(c).is_some() || DIGITS_NOT_DECIMAL.binary_search(&u32::from(c)).is_ok()
}

/// `unicodedata.decimal(c)`: a decimal digit's value.
pub fn decimal_value(c: char) -> Option<u32> {
    if !ferb_py::unicode::is_decimal(c) {
        return None;
    }
    let cp = u32::from(c);
    let at = DECIMAL_ZEROS.partition_point(|z| *z <= cp);
    let zero = DECIMAL_ZEROS[at.checked_sub(1)?];
    Some(cp - zero)
}
