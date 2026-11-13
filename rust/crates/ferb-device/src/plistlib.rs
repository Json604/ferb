//! Property lists as Python's `plistlib` writes and reads them: the LaunchAgent FERB writes must be byte for byte
//! what `plistlib.dumps` wrote (its SHA-256 is in `service.json`, which FERB.app compares, DMN-514).
//!
//! Writing: `plistlib.dumps(value)` (XML, keys sorted, one tab per level). Reading goes through the `plist` crate
//! (XML and binary, as `plistlib.loads` reads both).

use std::collections::BTreeMap;
use std::path::Path;

/// A value `plistlib.dumps` writes. Dict keys are sorted as Python's `sorted()` sorts `str` (code point order, which
/// is byte order for UTF-8).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Bool(bool),
    Integer(i64),
    Dict(BTreeMap<String, Value>),
    Array(Vec<Value>),
}

impl Value {
    pub fn string(s: impl Into<String>) -> Self {
        Value::String(s.into())
    }

    pub fn strings<I, S>(items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Value::Array(items.into_iter().map(|s| Value::String(s.into())).collect())
    }
}

/// `plistlib.dumps` refuses a string holding a control character other than tab and newlines.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("strings can't contain control characters; use bytes instead")]
pub struct ControlCharacter;

const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

/// `plistlib.dumps(value)` (FMT_XML, sort_keys=True).
pub fn dumps(value: &Value) -> Result<Vec<u8>, ControlCharacter> {
    let mut out = String::from(HEADER);
    out.push_str("<plist version=\"1.0\">\n");
    write_value(&mut out, value, 0)?;
    out.push_str("</plist>\n");
    Ok(out.into_bytes())
}

/// plistlib `_escape`.
fn escape(text: &str) -> Result<String, ControlCharacter> {
    if text
        .chars()
        .any(|c| matches!(c, '\x00'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f'))
    {
        return Err(ControlCharacter);
    }
    Ok(text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;"))
}

fn line(out: &mut String, level: usize, text: &str) {
    for _ in 0..level {
        out.push('\t');
    }
    out.push_str(text);
    out.push('\n');
}

fn simple(
    out: &mut String,
    level: usize,
    element: &str,
    text: Option<&str>,
) -> Result<(), ControlCharacter> {
    match text {
        Some(t) => line(
            out,
            level,
            &format!("<{element}>{}</{element}>", escape(t)?),
        ),
        None => line(out, level, &format!("<{element}/>")),
    }
    Ok(())
}

fn write_value(out: &mut String, value: &Value, level: usize) -> Result<(), ControlCharacter> {
    match value {
        Value::String(s) => simple(out, level, "string", Some(s)),
        Value::Bool(true) => simple(out, level, "true", None),
        Value::Bool(false) => simple(out, level, "false", None),
        Value::Integer(n) => simple(out, level, "integer", Some(&n.to_string())),
        Value::Dict(d) if d.is_empty() => simple(out, level, "dict", None),
        Value::Dict(d) => {
            line(out, level, "<dict>");
            for (k, v) in d {
                simple(out, level + 1, "key", Some(k))?;
                write_value(out, v, level + 1)?;
            }
            line(out, level, "</dict>");
            Ok(())
        }
        Value::Array(a) if a.is_empty() => simple(out, level, "array", None),
        Value::Array(a) => {
            line(out, level, "<array>");
            for v in a {
                write_value(out, v, level + 1)?;
            }
            line(out, level, "</array>");
            Ok(())
        }
    }
}

/// `plistlib.loads(bytes)`: None when the bytes aren't a property list (InvalidFileException).
pub fn loads(bytes: &[u8]) -> Option<plist::Value> {
    plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()
}

/// `plistlib.loads(path.read_bytes())`, None when the file can't be read or isn't a property list.
pub fn load(path: &Path) -> Option<plist::Value> {
    std::fs::read(path).ok().and_then(|b| loads(&b))
}

/// Python's `str(value)` for the plist values FERB reads as text (a string as it is, a number as its digits).
pub fn py_str(value: &plist::Value) -> String {
    match value {
        plist::Value::String(s) => s.clone(),
        plist::Value::Integer(i) => i
            .as_signed()
            .map(|n| n.to_string())
            .or_else(|| i.as_unsigned().map(|n| n.to_string()))
            .unwrap_or_default(),
        plist::Value::Boolean(true) => "True".into(),
        plist::Value::Boolean(false) => "False".into(),
        plist::Value::Real(f) => ferb_py::json::python_float(*f),
        other => format!("{other:?}"),
    }
}

/// Python truthiness of a plist value (`value or default`).
pub fn truthy(value: &plist::Value) -> bool {
    match value {
        plist::Value::String(s) => !s.is_empty(),
        plist::Value::Array(a) => !a.is_empty(),
        plist::Value::Dictionary(d) => !d.is_empty(),
        plist::Value::Boolean(b) => *b,
        plist::Value::Integer(i) => i.as_signed() != Some(0) && i.as_unsigned() != Some(0),
        plist::Value::Real(f) => *f != 0.0,
        plist::Value::Data(d) => !d.is_empty(),
        _ => true,
    }
}
