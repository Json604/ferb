//! The values a Python record holds, as redact.py walks them: `None`, `bool`, `int` of any size, `float`, `str`
//! (with the redactor's mark, redact.py:50), `bytes`, `list` and `dict`. A `serde_json::Value` cannot hold three
//! of those: an integer past 64 bits (it becomes a float and loses its digits), a marked string, and bytes. `Py`
//! reads JSON text as `json.loads` does and writes it as `json.dumps` does, so a record's integers keep every
//! digit through a redaction.
//!
//! Also Python's `str()`, `repr()` and truth of a value, for the places redact.py turns a record's fields into
//! text (`str(d.get("role", …))`, redact.py:300-330) or tests them with `or`.

use serde_json::Value;

use crate::known::json_escape;
use crate::unicode as u;

/// Which redactor marked a string, and under which known values and exclusions (redact.py:164
/// `Redactor.marked`): a mark made under other ones is never trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub(crate) redactor: u64,
    pub(crate) values: u64,
    pub(crate) config: u64,
}

/// A Python `str`, and the mark of the redactor that returned it (redact.py:50 `Redacted`). Equal when the text
/// is, as in Python.
#[derive(Clone, Debug, Default)]
pub struct PyStr {
    pub text: String,
    pub(crate) mark: Option<Mark>,
}

impl PyStr {
    /// A plain string: no redactor has returned it.
    pub fn new(text: impl Into<String>) -> PyStr {
        PyStr {
            text: text.into(),
            mark: None,
        }
    }

    pub(crate) fn marked(text: impl Into<String>, mark: Mark) -> PyStr {
        PyStr {
            text: text.into(),
            mark: Some(mark),
        }
    }

    /// Whether a redactor returned this string (which one, and whether its values changed since:
    /// `Redactor::is_redacted`).
    pub fn has_mark(&self) -> bool {
        self.mark.is_some()
    }
}

impl PartialEq for PyStr {
    fn eq(&self, other: &PyStr) -> bool {
        self.text == other.text
    }
}

/// A Python value (the module's note). `Int` holds `str(int)`: an optional "-" and digits with no leading zero.
/// A `Dict`'s keys are strings, each once, in insertion order (a Python dict with other keys, a tuple and a
/// bytearray have no form here: JSON has none).
#[derive(Clone, Debug, PartialEq)]
pub enum Py {
    None,
    Bool(bool),
    Int(String),
    Float(f64),
    Str(PyStr),
    Bytes(Vec<u8>),
    List(Vec<Py>),
    Dict(Vec<(PyStr, Py)>),
}

/// Why a text is not what `json.loads` reads into a `Py`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    /// Not JSON as Python's `json.loads` reads it.
    Syntax,
    /// A lone surrogate escape ("\\ud800"): Python keeps it in a `str`, a Rust `String` cannot.
    LoneSurrogate,
    /// An integer of more than 4300 digits: Python's `int()` refuses it (sys.int_info.default_max_str_digits).
    IntTooLong,
    /// Nested deeper than `MAX_DEPTH`.
    TooDeep,
    /// A value JSON cannot write: `bytes`.
    NotJson,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ferb-sanitize: not a JSON record ({self:?})")
    }
}

impl std::error::Error for JsonError {}

/// How deep `Py::from_json` reads (serde_json's own bound; Python's is its recursion limit, about 1000).
pub const MAX_DEPTH: usize = 128;
const MAX_INT_DIGITS: usize = 4300;

impl Py {
    pub fn str(text: impl Into<String>) -> Py {
        Py::Str(PyStr::new(text))
    }

    /// A Python `int` from its decimal text ("-12", "0"); None when it is not one.
    pub fn int(digits: &str) -> Option<Py> {
        let body = digits.strip_prefix('-').unwrap_or(digits);
        if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let body = body.trim_start_matches('0');
        Some(Py::Int(if body.is_empty() {
            "0".to_string()
        } else if digits.starts_with('-') {
            format!("-{body}")
        } else {
            body.to_string()
        }))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Py::Str(s) => Some(&s.text),
            _ => None,
        }
    }

    pub fn is_str(&self) -> bool {
        matches!(self, Py::Str(_))
    }

    /// `type(v) is int`: a bool is not one.
    pub fn is_int(&self) -> bool {
        matches!(self, Py::Int(_))
    }

    /// `d.get(key)` on a dict; None on anything else.
    pub fn get(&self, key: &str) -> Option<&Py> {
        match self {
            Py::Dict(d) => dict_get(d, key),
            _ => None,
        }
    }

    /// `json.loads(text)`: NaN and the infinities read too, a repeated key keeps its first place and its last
    /// value, an integer keeps every digit.
    pub fn from_json(text: &str) -> Result<Py, JsonError> {
        let mut p = Parser {
            s: text.as_bytes(),
            text,
            i: 0,
        };
        p.space();
        let out = p.value(0)?;
        p.space();
        if p.i == p.s.len() {
            Ok(out)
        } else {
            Err(JsonError::Syntax)
        }
    }

    /// `json.dumps(self, ensure_ascii=…)` (", " and ": " between items, floats as `repr`).
    pub fn to_json(&self, ensure_ascii: bool) -> Result<String, JsonError> {
        let mut out = String::new();
        self.dump(ensure_ascii, &mut out)?;
        Ok(out)
    }

    fn dump(&self, ascii: bool, out: &mut String) -> Result<(), JsonError> {
        match self {
            Py::None => out.push_str("null"),
            Py::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Py::Int(digits) => out.push_str(digits),
            Py::Float(f) if f.is_nan() => out.push_str("NaN"),
            Py::Float(f) if f.is_infinite() => {
                out.push_str(if *f < 0.0 { "-Infinity" } else { "Infinity" });
            }
            Py::Float(f) => out.push_str(&float_repr(*f)),
            Py::Str(s) => {
                out.push('"');
                out.push_str(&json_escape(&s.text, ascii));
                out.push('"');
            }
            Py::Bytes(_) => return Err(JsonError::NotJson),
            Py::List(items) => {
                out.push('[');
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        out.push_str(", ");
                    }
                    item.dump(ascii, out)?;
                }
                out.push(']');
            }
            Py::Dict(d) => {
                out.push('{');
                for (n, (k, v)) in d.iter().enumerate() {
                    if n > 0 {
                        out.push_str(", ");
                    }
                    out.push('"');
                    out.push_str(&json_escape(&k.text, ascii));
                    out.push_str("\": ");
                    v.dump(ascii, out)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
}

pub(crate) fn dict_get<'a>(d: &'a [(PyStr, Py)], key: &str) -> Option<&'a Py> {
    d.iter().find(|(k, _)| k.text == key).map(|(_, v)| v)
}

/// A JSON value as Python holds it. A number serde_json kept as a float stays one: an integer past 64 bits lost
/// its digits when serde_json read it (read the text with `Py::from_json` instead).
impl From<&Value> for Py {
    fn from(v: &Value) -> Py {
        match v {
            Value::Null => Py::None,
            Value::Bool(b) => Py::Bool(*b),
            Value::Number(n) if n.is_i64() || n.is_u64() => Py::Int(n.to_string()),
            Value::Number(n) => Py::Float(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => Py::str(s.as_str()),
            Value::Array(a) => Py::List(a.iter().map(Py::from).collect()),
            Value::Object(o) => Py::Dict(
                o.iter()
                    .map(|(k, v)| (PyStr::new(k.as_str()), Py::from(v)))
                    .collect(),
            ),
        }
    }
}

impl Py {
    /// Back to a `serde_json::Value`, for a value that came from one: an integer past 64 bits becomes a float,
    /// a float JSON cannot hold becomes null, bytes become a lossy string (none of them comes from a `Value`).
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Py::None => Value::Null,
            Py::Bool(b) => Value::Bool(*b),
            Py::Int(digits) => digits
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| digits.parse::<u64>().map(Value::from))
                .unwrap_or_else(|_| float_value(digits.parse::<f64>().unwrap_or(f64::NAN))),
            Py::Float(f) => float_value(*f),
            Py::Str(s) => Value::String(s.text.clone()),
            Py::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
            Py::List(items) => Value::Array(items.iter().map(Py::to_value).collect()),
            Py::Dict(d) => Value::Object(
                d.iter()
                    .map(|(k, v)| (k.text.clone(), v.to_value()))
                    .collect(),
            ),
        }
    }
}

fn float_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn word(&mut self, word: &str) -> bool {
        let found = self.s[self.i..].starts_with(word.as_bytes());
        if found {
            self.i += word.len();
        }
        found
    }

    fn value(&mut self, depth: usize) -> Result<Py, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::TooDeep);
        }
        match self.s.get(self.i).copied() {
            Some(b'"') => Ok(Py::str(self.string()?)),
            Some(b'{') => self.dict(depth),
            Some(b'[') => self.list(depth),
            Some(b'n') if self.word("null") => Ok(Py::None),
            Some(b't') if self.word("true") => Ok(Py::Bool(true)),
            Some(b'f') if self.word("false") => Ok(Py::Bool(false)),
            Some(b'N') if self.word("NaN") => Ok(Py::Float(f64::NAN)),
            Some(b'I') if self.word("Infinity") => Ok(Py::Float(f64::INFINITY)),
            Some(b'-') if self.word("-Infinity") => Ok(Py::Float(f64::NEG_INFINITY)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(JsonError::Syntax),
        }
    }

    fn digits(&mut self) -> usize {
        let from = self.i;
        while matches!(self.s.get(self.i), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - from
    }

    /// json/scanner.py NUMBER_RE: `(-?(?:0|[1-9][0-9]*))(\.[0-9]+)?([eE][-+]?[0-9]+)?`.
    fn number(&mut self) -> Result<Py, JsonError> {
        let from = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        let int_digits = if self.s.get(self.i) == Some(&b'0') {
            self.i += 1;
            1
        } else {
            self.digits()
        };
        if int_digits == 0 {
            return Err(JsonError::Syntax);
        }
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') && matches!(self.s.get(self.i + 1), Some(b'0'..=b'9'))
        {
            self.i += 1;
            self.digits();
            float = true;
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            let mark = self.i;
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                self.i = mark; // no exponent after all: the number ends before the "e"
            } else {
                float = true;
            }
        }
        let text = &self.text[from..self.i];
        if float {
            return text
                .parse::<f64>()
                .map(Py::Float)
                .map_err(|_| JsonError::Syntax);
        }
        if int_digits > MAX_INT_DIGITS {
            return Err(JsonError::IntTooLong);
        }
        Py::int(text).ok_or(JsonError::Syntax)
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let hex = self.s.get(self.i..self.i + 4).ok_or(JsonError::Syntax)?;
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(JsonError::Syntax);
        }
        let text = std::str::from_utf8(hex).map_err(|_| JsonError::Syntax)?;
        self.i += 4;
        u32::from_str_radix(text, 16).map_err(|_| JsonError::Syntax)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let from = self.i;
            while let Some(&b) = self.s.get(self.i) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(&self.text[from..self.i]);
            match self.s.get(self.i).copied() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    let esc = self.s.get(self.i + 1).copied().ok_or(JsonError::Syntax)?;
                    self.i += 2;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xd800..0xdc00).contains(&cp)
                                && self.s[self.i..].starts_with(b"\\u")
                            {
                                let mark = self.i;
                                self.i += 2;
                                let low = self.hex4()?;
                                if (0xdc00..0xe000).contains(&low) {
                                    cp = 0x10000 + ((cp - 0xd800) << 10) + (low - 0xdc00);
                                } else {
                                    self.i = mark;
                                }
                            }
                            out.push(char::from_u32(cp).ok_or(JsonError::LoneSurrogate)?);
                        }
                        _ => return Err(JsonError::Syntax),
                    }
                }
                _ => return Err(JsonError::Syntax), // a control character, or the text ended
            }
        }
    }

    fn list(&mut self, depth: usize) -> Result<Py, JsonError> {
        self.i += 1;
        let mut out = Vec::new();
        self.space();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Py::List(out));
        }
        loop {
            self.space();
            out.push(self.value(depth + 1)?);
            self.space();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Py::List(out));
                }
                _ => return Err(JsonError::Syntax),
            }
        }
    }

    fn dict(&mut self, depth: usize) -> Result<Py, JsonError> {
        self.i += 1;
        let mut out: Vec<(PyStr, Py)> = Vec::new();
        self.space();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Py::Dict(out));
        }
        loop {
            self.space();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(JsonError::Syntax);
            }
            let key = self.string()?;
            self.space();
            if self.s.get(self.i) != Some(&b':') {
                return Err(JsonError::Syntax);
            }
            self.i += 1;
            self.space();
            let value = self.value(depth + 1)?;
            match out.iter_mut().find(|(k, _)| k.text == key) {
                Some(entry) => entry.1 = value,
                None => out.push((PyStr::new(key), value)),
            }
            self.space();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Py::Dict(out));
                }
                _ => return Err(JsonError::Syntax),
            }
        }
    }
}

/// `bool(value)`.
pub fn truthy(v: &Py) -> bool {
    match v {
        Py::None => false,
        Py::Bool(b) => *b,
        Py::Int(digits) => digits != "0",
        Py::Float(f) => *f != 0.0,
        Py::Str(s) => !s.text.is_empty(),
        Py::Bytes(b) => !b.is_empty(),
        Py::List(a) => !a.is_empty(),
        Py::Dict(o) => !o.is_empty(),
    }
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

/// `repr(bytes)`.
pub fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::with_capacity(b.len() + 3);
    out.push('b');
    out.push(quote as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            _ => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// `repr(value)`.
pub fn repr(v: &Py) -> String {
    match v {
        Py::None => "None".into(),
        Py::Bool(true) => "True".into(),
        Py::Bool(false) => "False".into(),
        Py::Int(digits) => digits.clone(),
        Py::Float(f) => float_repr(*f),
        Py::Str(s) => str_repr(&s.text),
        Py::Bytes(b) => bytes_repr(b),
        Py::List(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Py::Dict(o) => {
            format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{}: {}", str_repr(&k.text), repr(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// `str(value)`.
pub fn to_str(v: &Py) -> String {
    match v {
        Py::Str(s) => s.text.clone(),
        other => repr(other),
    }
}
