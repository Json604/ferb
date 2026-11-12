//! Python's `str()`, `repr()` and truth of a JSON value, for the places redact.py turns a record's fields into
//! text (`str(d.get("role", …))`, redact.py:300-330) or tests them with `or`.

use serde_json::Value;

use crate::unicode as u;

/// `bool(value)`.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            n.as_f64().is_some_and(|f| f != 0.0) || n.as_u64().is_some_and(|x| x != 0)
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A JSON number Python holds as an `int` (written without ".", "e" or "E").
pub fn is_int(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_i64() || n.is_u64())
}

/// `repr(float)`.
pub fn float_repr(f: f64) -> String {
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let sci = format!("{:e}", f.abs()); // the shortest digits that read back: "1.2345e-5"
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let decpt: i32 = exp.parse::<i32>().unwrap_or(0) + 1;
    let n = digits.len() as i32;
    let body = if decpt <= -4 || decpt > 16 {
        let e = decpt - 1;
        let frac = if n > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!(
            "{}{}e{}{:02}",
            &digits[..1],
            frac,
            if e < 0 { '-' } else { '+' },
            e.abs()
        )
    } else if decpt <= 0 {
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt >= n {
        format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
    } else {
        format!(
            "{}.{}",
            &digits[..decpt as usize],
            &digits[decpt as usize..]
        )
    };
    if f < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

/// `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if cp < 0x20 || cp == 0x7f => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp < 0x7f || u::is_printable(c) => out.push(c),
            _ if cp < 0x100 => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp < 0x10000 => out.push_str(&format!("\\u{cp:04x}")),
            _ => out.push_str(&format!("\\U{cp:08x}")),
        }
    }
    out.push(quote);
    out
}

/// `repr(value)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => {
            if is_int(v) {
                n.to_string()
            } else {
                float_repr(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => str_repr(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{}: {}", str_repr(k), repr(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// `str(value)`.
pub fn to_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}
