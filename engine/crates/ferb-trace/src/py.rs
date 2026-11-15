//! Python's behavior on decoded JSON values and text, where the trace code leans on it: `dict.get` chains, `or`,
//! `str()` and `repr()` of any value, `splitlines`, `round`, `datetime` arithmetic and `strftime`.
//!
//! A value that is not the object Python expects reads as one without keys where Python would raise.

use std::sync::LazyLock;

use ferb_py::time::DateTime;
use serde_json::{Map, Value};

use crate::printable::NOT_PRINTABLE;

pub use ferb_py::value::{char_len, is_py_space, prefix, squeeze, strip, truthy};

pub static NULL: Value = Value::Null;
pub static EMPTY: LazyLock<Value> = LazyLock::new(|| Value::Object(Map::new()));
pub static EMPTY_LIST: LazyLock<Value> = LazyLock::new(|| Value::Array(Vec::new()));

/// `value.get(key)`: the stored value, `null` for a missing key or a value without keys.
pub fn g<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.as_object().and_then(|m| m.get(key)).unwrap_or(&NULL)
}

/// `value.get(key, default)`: the default only when the key is missing (a stored `null` stays `null`).
pub fn gd<'a>(v: &'a Value, key: &str, default: &'a Value) -> &'a Value {
    v.as_object().and_then(|m| m.get(key)).unwrap_or(default)
}

/// `key in value` for an object.
pub fn has(v: &Value, key: &str) -> bool {
    v.as_object().is_some_and(|m| m.contains_key(key))
}

/// `a or b`.
pub fn or<'a>(a: &'a Value, b: &'a Value) -> &'a Value {
    if truthy(a) {
        a
    } else {
        b
    }
}

/// `value.get(key) or {}`.
pub fn obj<'a>(v: &'a Value, key: &str) -> &'a Value {
    or(g(v, key), &EMPTY)
}

/// `value.get(key) or []`, as the items Python would iterate.
pub fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    match or(g(v, key), &EMPTY_LIST) {
        Value::Array(a) => a,
        _ => &[],
    }
}

pub fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    g(v, key).as_str().unwrap_or("")
}

pub fn string(s: &str) -> Value {
    Value::String(s.to_owned())
}

/// `isinstance(value, int)` (a bool is an int).
pub fn is_int(v: &Value) -> bool {
    match v {
        Value::Bool(_) => true,
        Value::Number(n) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

/// An int's value, for a value `is_int` accepts.
pub fn int_val(v: &Value) -> i64 {
    match v {
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => n.as_i64().unwrap_or_else(|| {
            n.as_u64()
                .map_or(i64::MAX, |u| u.min(i64::MAX as u64) as i64)
        }),
        _ => 0,
    }
}

/// `isinstance(value, (int, float))`.
pub fn is_number(v: &Value) -> bool {
    matches!(v, Value::Bool(_) | Value::Number(_))
}

/// A number as a float (bool as 0 or 1).
pub fn num(v: &Value) -> f64 {
    match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `int(value)`: text read as a whole number, a float cut towards zero, `None` and false as 0. What Python raises on
/// (text that is no number) reads as 0.
pub fn int_of(v: &Value) -> i64 {
    match v {
        Value::Null => 0,
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => n.as_i64().unwrap_or_else(|| {
            n.as_u64().map_or_else(
                || n.as_f64().unwrap_or(0.0) as i64,
                |u| u.min(i64::MAX as u64) as i64,
            )
        }),
        Value::String(s) => strip(s).replace('_', "").parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// `round(x)`: half to even.
pub fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `round(x, digits)`: the correctly rounded decimal, read back.
pub fn round_digits(x: f64, digits: usize) -> f64 {
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// `str(value)`.
pub fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

/// `repr(value)` of a decoded JSON value.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(n) => ferb_py::json::number(n),
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn printable(c: char) -> bool {
    let cp = c as u32;
    NOT_PRINTABLE
        .binary_search_by(|&(a, b)| {
            if cp < a {
                std::cmp::Ordering::Greater
            } else if cp > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
}

/// `repr(text)`.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_ascii() || printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `text.splitlines()`.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        out.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' && matches!(it.peek(), Some((_, '\n'))) {
            it.next();
            end += 1;
        }
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `text.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_py_space)
}

/// `json.loads(text)` for a text Python reads; `None` when it raises (or serde cannot read it).
pub fn loads(text: &str) -> Option<Value> {
    ferb_py::json::check(text).ok()?;
    ferb_py::json::loads(text).ok()
}

/// `_json` of the trace code: a JSON text read as an object, an object as it is, anything else `{}`.
pub fn json_obj(v: &Value) -> Value {
    let parsed = match v {
        Value::String(s) => loads(s).unwrap_or(Value::Null),
        other => other.clone(),
    };
    if parsed.is_object() {
        parsed
    } else {
        EMPTY.clone()
    }
}

/// `datetime.fromisoformat(text)`.
pub fn parse_time(text: &str) -> Option<DateTime> {
    DateTime::from_isoformat(text).ok()
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Microseconds since the epoch, an aware time moved to UTC (a naive one is read as UTC: Python refuses to compare
/// the two).
pub fn micros(t: &DateTime) -> i128 {
    let days = i128::from(days_from_civil(
        i64::from(t.year),
        i64::from(t.month),
        i64::from(t.day),
    ));
    let wall = days * 86_400_000_000
        + i128::from(t.hour) * 3_600_000_000
        + i128::from(t.minute) * 60_000_000
        + i128::from(t.second) * 1_000_000
        + i128::from(t.microsecond);
    match t.offset {
        Some((s, us)) => wall - (i128::from(s) * 1_000_000 + i128::from(us)),
        None => wall,
    }
}

/// `(end - start).total_seconds()`.
pub fn total_seconds(start: &str, end: &str) -> Option<f64> {
    let (a, b) = (parse_time(start)?, parse_time(end)?);
    Some((micros(&b) - micros(&a)) as f64 / 1_000_000.0)
}

/// `datetime.fromisoformat(at).strftime("%H:%M:%S")`.
pub fn hms(at: &str) -> String {
    match parse_time(at) {
        Some(t) => format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second),
        None => "--:--:--".to_owned(),
    }
}

/// `datetime.fromisoformat(at).strftime("%d %b %H:%M")`.
pub fn day_month_time(at: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    match parse_time(at) {
        Some(t) => format!(
            "{:02} {} {:02}:{:02}",
            t.day,
            MONTHS[(t.month as usize - 1) % 12],
            t.hour,
            t.minute
        ),
        None => String::new(),
    }
}

/// A new object with the given keys in order.
pub fn map(entries: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in entries {
        m.insert(k.to_owned(), v);
    }
    Value::Object(m)
}

/// A float as the JSON number Python writes (null for NaN and infinity, which FERB never records).
pub fn float(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

pub fn opt_float(x: Option<f64>) -> Value {
    x.map_or(Value::Null, float)
}
