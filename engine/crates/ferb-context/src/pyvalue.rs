//! A model's answer as Python holds it after `json.loads`, and the text Python makes of it: `repr` (inside
//! jsonschema's messages, structured.py:26) and `json.dumps` (a mapped-back answer, schema_shape.py:77; the stated
//! form, schema_shape.py:295; the refusal key, schema_shape.py:272).
//!
//! A `serde_json::Value` loses what an answer can hold and Python keeps: an integer of any size, NaN and the
//! infinities, and the difference between `1` and `1.0`. `Py` keeps them. Reading, writing, `repr`, cloning and
//! dropping are iterative, so an answer nested thousands deep is handled as Python handles it, up to `MAX_DEPTH`
//! (where Python's own scanner gives up with a `RecursionError`).
//!
//! One difference is kept and listed (journal): a lone surrogate escape (`"\ud800"`) reads as U+FFFD, since a Rust
//! `String` cannot hold one; Python keeps it and `repr` writes `'\ud800'`.

use std::fmt::Write as _;

use indexmap::IndexMap;
use serde_json::Value;

use crate::generated::NOT_PRINTABLE;

/// A Python value from JSON: `None`, `bool`, `int` (its decimal text: an optional "-" and digits with no leading
/// zero), `float`, `str`, `list` and `dict` (string keys, insertion order).
#[derive(Debug, PartialEq)]
pub enum Py {
    None,
    Bool(bool),
    Int(String),
    Float(f64),
    Str(String),
    List(List),
    Dict(Dict),
}

/// A Python `list`. Dropped without recursion, so a value nested any depth never exhausts the stack.
#[derive(Debug, Default, PartialEq)]
pub struct List(pub Vec<Py>);

/// A Python `dict` with string keys, in insertion order. Dropped without recursion.
#[derive(Debug, Default, PartialEq)]
pub struct Dict(pub IndexMap<String, Py>);

impl Drop for List {
    fn drop(&mut self) {
        dismantle(std::mem::take(&mut self.0));
    }
}

impl Drop for Dict {
    fn drop(&mut self) {
        dismantle(std::mem::take(&mut self.0).into_values().collect());
    }
}

/// Drops nested values one level at a time instead of recursively.
fn dismantle(mut stack: Vec<Py>) {
    while let Some(value) = stack.pop() {
        match value {
            Py::List(mut list) => stack.append(&mut list.0),
            Py::Dict(mut dict) => stack.extend(std::mem::take(&mut dict.0).into_values()),
            _ => {}
        }
    }
}

impl Clone for Py {
    /// Iterative, like the rest: a deep value clones without recursion.
    fn clone(&self) -> Py {
        enum Step<'a> {
            Visit(&'a Py),
            CloseList(usize),
            CloseDict(Vec<&'a String>),
        }
        let mut work = vec![Step::Visit(self)];
        let mut done: Vec<Py> = Vec::new();
        while let Some(step) = work.pop() {
            match step {
                Step::Visit(Py::List(list)) => {
                    work.push(Step::CloseList(list.0.len()));
                    work.extend(list.0.iter().rev().map(Step::Visit));
                }
                Step::Visit(Py::Dict(dict)) => {
                    work.push(Step::CloseDict(dict.0.keys().collect()));
                    work.extend(dict.0.values().rev().map(Step::Visit));
                }
                Step::Visit(leaf) => done.push(match leaf {
                    Py::None => Py::None,
                    Py::Bool(b) => Py::Bool(*b),
                    Py::Int(i) => Py::Int(i.clone()),
                    Py::Float(f) => Py::Float(*f),
                    Py::Str(s) => Py::Str(s.clone()),
                    Py::List(_) | Py::Dict(_) => unreachable!("containers are visited above"),
                }),
                Step::CloseList(n) => {
                    let items = done.split_off(done.len() - n);
                    done.push(Py::List(List(items)));
                }
                Step::CloseDict(keys) => {
                    let values = done.split_off(done.len() - keys.len());
                    let map = keys.into_iter().cloned().zip(values).collect();
                    done.push(Py::Dict(Dict(map)));
                }
            }
        }
        done.pop().expect("one value in, one value out")
    }
}

impl Py {
    pub fn str(text: impl Into<String>) -> Py {
        Py::Str(text.into())
    }

    pub fn list(items: Vec<Py>) -> Py {
        Py::List(List(items))
    }

    pub fn dict(entries: impl IntoIterator<Item = (String, Py)>) -> Py {
        Py::Dict(Dict(entries.into_iter().collect()))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Py::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&IndexMap<String, Py>> {
        match self {
            Py::Dict(d) => Some(&d.0),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Py]> {
        match self {
            Py::List(l) => Some(&l.0),
            _ => None,
        }
    }

    /// `d.get(key)` on a dict; None on anything else.
    pub fn get(&self, key: &str) -> Option<&Py> {
        self.as_dict().and_then(|d| d.get(key))
    }

    /// The JSON type name schema_shape.py:314 `_type_of` gives: "null", "boolean", "object", "array", "string" or
    /// "number" (an int is a "number" there).
    pub fn type_of(&self) -> &'static str {
        match self {
            Py::None => "null",
            Py::Bool(_) => "boolean",
            Py::Dict(_) => "object",
            Py::List(_) => "array",
            Py::Str(_) => "string",
            Py::Int(_) | Py::Float(_) => "number",
        }
    }

    /// A schema or other `serde_json::Value` as Python's `json.loads` would have given it.
    pub fn from_value(value: &Value) -> Py {
        match value {
            Value::Null => Py::None,
            Value::Bool(b) => Py::Bool(*b),
            Value::Number(n) if n.is_i64() || n.is_u64() => Py::Int(n.to_string()),
            Value::Number(n) => Py::Float(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => Py::Str(s.clone()),
            Value::Array(items) => Py::list(items.iter().map(Py::from_value).collect()),
            Value::Object(map) => Py::dict(map.iter().map(|(k, v)| (k.clone(), Py::from_value(v)))),
        }
    }

    /// Back to a `serde_json::Value`: an integer past 64 bits becomes a float, NaN and the infinities null (JSON
    /// has neither). For callers that need a `Value`; FERB's own text comes from `dumps`.
    pub fn to_value(&self) -> Value {
        match self {
            Py::None => Value::Null,
            Py::Bool(b) => Value::Bool(*b),
            Py::Int(digits) => digits
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| digits.parse::<u64>().map(Value::from))
                .unwrap_or_else(|_| float_value(digits.parse::<f64>().unwrap_or(f64::NAN))),
            Py::Float(f) => float_value(*f),
            Py::Str(s) => Value::String(s.clone()),
            Py::List(items) => Value::Array(items.0.iter().map(Py::to_value).collect()),
            Py::Dict(map) => Value::Object(
                map.0
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_value()))
                    .collect(),
            ),
        }
    }
}

fn float_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}

// ---------------------------------------------------------------------------------------------------------------
// json.loads
// ---------------------------------------------------------------------------------------------------------------

/// Python's limit on an `int`'s digits (sys.int_info.default_max_str_digits): `json.loads` refuses a longer one.
pub const MAX_INT_DIGITS: usize = 4300;

/// How deep `loads` reads. Python's scanner raises `RecursionError` (not a `ValueError`) at about 10,000 levels,
/// where exactly depending on its own call stack; past this depth `loads` gives `LoadError::TooDeep`.
pub const MAX_DEPTH: usize = 9_990;

/// What `json.loads` raises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// A `ValueError`: the `JSONDecodeError`'s `msg` (no position), or an over-long integer's text.
    Value(String),
    /// Nested past `MAX_DEPTH` (Python: `RecursionError`).
    TooDeep,
}

/// `json.loads(text)` on a `str`. A repeated key keeps its first place and its last value; NaN and the infinities
/// read as floats; an integer keeps every digit.
pub fn loads(text: &str) -> Result<Py, LoadError> {
    let mut parser = Parser::new(text);
    let read = parser.document();
    if parser.deepest > MAX_DEPTH {
        return Err(LoadError::TooDeep);
    }
    match read {
        Ok(value) => Ok(value),
        Err(Stop::IntTooLong(digits)) => Err(LoadError::Value(int_too_long(digits))),
        Err(Stop::Syntax) => Err(LoadError::Value(syntax_message(text, parser.deepest))),
    }
}

/// Python's words for a syntax error (ferb-py follows its C scanner). ferb-py's scanner recurses once per level, so
/// a deep text is scanned on a thread with room for `MAX_DEPTH` levels.
fn syntax_message(text: &str, depth: usize) -> String {
    let scan = |text: &str| ferb_py::json::check(text).err();
    let message = if depth <= 1000 {
        scan(text)
    } else {
        let owned = text.to_string();
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || scan(&owned))
            .ok()
            .and_then(|h| h.join().ok())
            .flatten()
    };
    match message {
        Some(m) => match m.rsplit_once(": line ") {
            Some((msg, _)) => msg.to_string(),
            None => m,
        },
        // Both scanners follow Python's; one refusing what the other reads would be a port bug.
        None => "Expecting value".to_string(),
    }
}

fn int_too_long(digits: usize) -> String {
    format!(
        "Exceeds the limit ({MAX_INT_DIGITS} digits) for integer string conversion: value has {digits} digits; \
         use sys.set_int_max_str_digits() to increase the limit"
    )
}

enum Stop {
    Syntax,
    IntTooLong(usize),
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    /// the most containers open at once so far
    deepest: usize,
}

/// Where a container being read is in the iterative reader.
enum Open {
    List(Vec<Py>),
    Dict(IndexMap<String, Py>, String),
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Parser<'a> {
        Parser {
            s: text.as_bytes(),
            text,
            i: 0,
            deepest: 0,
        }
    }

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

    fn document(&mut self) -> Result<Py, Stop> {
        self.space();
        let value = self.value()?;
        self.space();
        if self.i == self.s.len() {
            Ok(value)
        } else {
            Err(Stop::Syntax)
        }
    }

    /// One value, its containers read with an explicit stack.
    fn value(&mut self) -> Result<Py, Stop> {
        let mut stack: Vec<Open> = Vec::new();
        loop {
            // read a value, or open a container
            let mut value = match self.s.get(self.i).copied() {
                Some(b'[') => {
                    self.i += 1;
                    self.deepest = self.deepest.max(stack.len() + 1);
                    self.space();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        Py::list(Vec::new())
                    } else {
                        stack.push(Open::List(Vec::new()));
                        self.deepest = self.deepest.max(stack.len());
                        continue;
                    }
                }
                Some(b'{') => {
                    self.i += 1;
                    self.space();
                    if self.s.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        Py::dict([])
                    } else {
                        self.deepest = self.deepest.max(stack.len() + 1);
                        let key = self.key()?;
                        stack.push(Open::Dict(IndexMap::new(), key));
                        continue;
                    }
                }
                _ => self.scalar()?,
            };
            // close every container this value completes
            loop {
                self.space();
                match stack.last_mut() {
                    None => return Ok(value),
                    Some(Open::List(items)) => {
                        items.push(value);
                        match self.s.get(self.i) {
                            Some(b',') => {
                                self.i += 1;
                                self.space();
                                break;
                            }
                            Some(b']') => {
                                self.i += 1;
                                let Some(Open::List(items)) = stack.pop() else {
                                    unreachable!("the top was a list")
                                };
                                value = Py::list(items);
                            }
                            _ => return Err(Stop::Syntax),
                        }
                    }
                    Some(Open::Dict(map, key)) => {
                        map.insert(std::mem::take(key), value);
                        match self.s.get(self.i) {
                            Some(b',') => {
                                self.i += 1;
                                self.space();
                                *key = self.key()?;
                                break;
                            }
                            Some(b'}') => {
                                self.i += 1;
                                let Some(Open::Dict(map, _)) = stack.pop() else {
                                    unreachable!("the top was a dict")
                                };
                                value = Py::Dict(Dict(map));
                            }
                            _ => return Err(Stop::Syntax),
                        }
                    }
                }
            }
        }
    }

    /// `"key"`, then `:`, positioned at the value.
    fn key(&mut self) -> Result<String, Stop> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(Stop::Syntax);
        }
        let key = self.string()?;
        self.space();
        if self.s.get(self.i) != Some(&b':') {
            return Err(Stop::Syntax);
        }
        self.i += 1;
        self.space();
        Ok(key)
    }

    fn scalar(&mut self) -> Result<Py, Stop> {
        match self.s.get(self.i).copied() {
            Some(b'"') => Ok(Py::Str(self.string()?)),
            Some(b'n') if self.word("null") => Ok(Py::None),
            Some(b't') if self.word("true") => Ok(Py::Bool(true)),
            Some(b'f') if self.word("false") => Ok(Py::Bool(false)),
            Some(b'N') if self.word("NaN") => Ok(Py::Float(f64::NAN)),
            Some(b'I') if self.word("Infinity") => Ok(Py::Float(f64::INFINITY)),
            Some(b'-') if self.word("-Infinity") => Ok(Py::Float(f64::NEG_INFINITY)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(Stop::Syntax),
        }
    }

    /// `_match_number_unicode`: an int unless a fraction or an exponent follows.
    fn number(&mut self) -> Result<Py, Stop> {
        let start = self.i;
        let digit = |p: &Parser, at: usize| p.s.get(at).is_some_and(u8::is_ascii_digit);
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while digit(self, self.i) {
                    self.i += 1;
                }
            }
            _ => return Err(Stop::Syntax),
        }
        let int_end = self.i;
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') && digit(self, self.i + 1) {
            float = true;
            self.i += 2;
            while digit(self, self.i) {
                self.i += 1;
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            let mut e = self.i + 1;
            if matches!(self.s.get(e), Some(b'+' | b'-')) {
                e += 1;
            }
            if digit(self, e) {
                while digit(self, e) {
                    e += 1;
                }
                float = true;
                self.i = e;
            }
        }
        let text = &self.text[start..self.i];
        if float {
            // Rust's parse rounds correctly, as Python's float() does; past the range it gives ±inf, as Python does
            return Ok(Py::Float(text.parse::<f64>().map_err(|_| Stop::Syntax)?));
        }
        let body = &self.text[start..int_end];
        let (negative, digits) = match body.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, body),
        };
        if digits.len() > MAX_INT_DIGITS {
            return Err(Stop::IntTooLong(digits.len()));
        }
        Ok(Py::Int(if digits == "0" || !negative {
            digits.to_string()
        } else {
            format!("-{digits}")
        }))
    }

    /// A string at `"`; escapes decoded, a surrogate pair joined, a lone surrogate read as U+FFFD (module note).
    fn string(&mut self) -> Result<String, Stop> {
        self.i += 1;
        let mut out = String::new();
        let mut plain = self.i;
        loop {
            match self.s.get(self.i).copied() {
                None => return Err(Stop::Syntax),
                Some(b'"') => {
                    out.push_str(&self.text[plain..self.i]);
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    out.push_str(&self.text[plain..self.i]);
                    self.i += 1;
                    let c = self.s.get(self.i).copied().ok_or(Stop::Syntax)?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            if (0xD800..0xDC00).contains(&unit)
                                && self.s.get(self.i) == Some(&b'\\')
                                && self.s.get(self.i + 1) == Some(&b'u')
                            {
                                let save = self.i;
                                self.i += 2;
                                let low = self.hex4()?;
                                if (0xDC00..0xE000).contains(&low) {
                                    let code = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                                } else {
                                    // the second escape is read again on its own (scanner.c)
                                    self.i = save;
                                    out.push('\u{fffd}');
                                }
                            } else {
                                out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err(Stop::Syntax),
                    }
                    plain = self.i;
                }
                Some(c) if c < 0x20 => return Err(Stop::Syntax),
                Some(_) => self.i += 1,
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Stop> {
        let digits = self.s.get(self.i..self.i + 4).ok_or(Stop::Syntax)?;
        let mut code = 0u32;
        for &d in digits {
            code = (code << 4) | char::from(d).to_digit(16).ok_or(Stop::Syntax)?;
        }
        self.i += 4;
        Ok(code)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// repr
// ---------------------------------------------------------------------------------------------------------------

/// `str.isprintable()` for one character (Python 3.12's tables, Unicode 15.0.0, generated by make.py).
pub fn is_printable(c: char) -> bool {
    let cp = c as u32;
    let at = NOT_PRINTABLE.partition_point(|&(_, hi)| hi < cp);
    !NOT_PRINTABLE.get(at).is_some_and(|&(lo, _)| lo <= cp)
}

/// `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_str_repr(s, &mut out);
    out
}

fn write_str_repr(s: &str, out: &mut String) {
    ferb_py::text::write_str_repr(s, out)
}

/// `repr(float)`, as `float.__repr__` writes it (`1e+16`, `2.0`, `nan`, `-inf`).
pub fn float_repr(f: f64) -> String {
    ferb_py::json::python_float(f)
}

/// `repr(value)`: `None`, `True`, `'text'`, `[1, 2.0]`, `{'k': None}`.
pub fn repr(value: &Py) -> String {
    let mut out = String::new();
    write_tree(value, &mut out, &Writer::Repr);
    out
}

/// `str(value)`: a string as it is, anything else as `repr`.
pub fn to_str(value: &Py) -> String {
    match value {
        Py::Str(s) => s.clone(),
        other => repr(other),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// json.dumps
// ---------------------------------------------------------------------------------------------------------------

/// The arguments one `json.dumps` call site passes.
#[derive(Clone, Copy, Debug)]
pub struct Dumps {
    /// `ensure_ascii` (Python's default: true).
    pub ascii: bool,
    /// `separators=(",", ":")` when true; `(", ", ": ")` (the default) otherwise.
    pub compact: bool,
    pub sort_keys: bool,
}

impl Dumps {
    /// `json.dumps(x)`.
    pub const DEFAULT: Dumps = Dumps {
        ascii: true,
        compact: false,
        sort_keys: false,
    };
    /// `json.dumps(x, ensure_ascii=False)`.
    pub const UTF8: Dumps = Dumps {
        ascii: false,
        compact: false,
        sort_keys: false,
    };
    /// `json.dumps(x, ensure_ascii=False, separators=(",", ":"))`.
    pub const COMPACT_UTF8: Dumps = Dumps {
        ascii: false,
        compact: true,
        sort_keys: false,
    };
}

/// `json.dumps(value, …)`: floats as `float.__repr__`, NaN and the infinities as `NaN`, `Infinity`, `-Infinity`
/// (`allow_nan` is Python's default), keys sorted by code point with `sort_keys`.
pub fn dumps(value: &Py, how: Dumps) -> String {
    let mut out = String::new();
    write_tree(value, &mut out, &Writer::Json(how));
    out
}

/// `json.dumps` of a `serde_json::Value` (a schema), with any of the call sites' arguments.
pub fn dumps_value(value: &Value, how: Dumps) -> String {
    dumps(&Py::from_value(value), how)
}

enum Writer {
    Repr,
    Json(Dumps),
}

/// Writes `value` as `repr` or `json.dumps` writes it, with an explicit stack instead of recursion.
fn write_tree(value: &Py, out: &mut String, writer: &Writer) {
    enum Step<'a> {
        Value(&'a Py),
        Key(&'a str),
        Text(&'static str),
    }
    let (item, key) = match writer {
        Writer::Json(Dumps { compact: true, .. }) => (",", ":"),
        _ => (", ", ": "),
    };
    let mut stack = vec![Step::Value(value)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Text(text) => out.push_str(text),
            Step::Key(k) => {
                match writer {
                    Writer::Repr => write_str_repr(k, out),
                    Writer::Json(how) => write_json_str(k, how.ascii, out),
                }
                out.push_str(key);
            }
            Step::Value(Py::List(items)) => {
                out.push('[');
                stack.push(Step::Text("]"));
                for (n, v) in items.0.iter().enumerate().rev() {
                    stack.push(Step::Value(v));
                    if n > 0 {
                        stack.push(Step::Text(item));
                    }
                }
            }
            Step::Value(Py::Dict(map)) => {
                out.push('{');
                stack.push(Step::Text("}"));
                let mut entries: Vec<(&String, &Py)> = map.0.iter().collect();
                if let Writer::Json(Dumps {
                    sort_keys: true, ..
                }) = writer
                {
                    entries.sort_by(|a, b| a.0.cmp(b.0));
                }
                for (n, (k, v)) in entries.into_iter().enumerate().rev() {
                    stack.push(Step::Value(v));
                    stack.push(Step::Key(k));
                    if n > 0 {
                        stack.push(Step::Text(item));
                    }
                }
            }
            Step::Value(leaf) => write_leaf(leaf, out, writer),
        }
    }
}

fn write_leaf(value: &Py, out: &mut String, writer: &Writer) {
    match (value, writer) {
        (Py::None, Writer::Repr) => out.push_str("None"),
        (Py::None, Writer::Json(_)) => out.push_str("null"),
        (Py::Bool(b), Writer::Repr) => out.push_str(if *b { "True" } else { "False" }),
        (Py::Bool(b), Writer::Json(_)) => out.push_str(if *b { "true" } else { "false" }),
        (Py::Int(digits), _) => out.push_str(digits),
        (Py::Float(f), Writer::Json(_)) if f.is_nan() => out.push_str("NaN"),
        (Py::Float(f), Writer::Json(_)) if f.is_infinite() => {
            out.push_str(if *f < 0.0 { "-Infinity" } else { "Infinity" })
        }
        (Py::Float(f), _) => out.push_str(&float_repr(*f)),
        (Py::Str(s), Writer::Repr) => write_str_repr(s, out),
        (Py::Str(s), Writer::Json(how)) => write_json_str(s, how.ascii, out),
        (Py::List(_) | Py::Dict(_), _) => unreachable!("containers are written by write_tree"),
    }
}

/// A JSON string as `json.dumps` writes it: `\"`, `\\`, `\n`, `\r`, `\t`, `\b`, `\f`, other control characters as
/// `\u00XX`; with `ensure_ascii` everything past `~` too (U+007F included), as `\uXXXX` or a surrogate pair.
fn write_json_str(s: &str, ascii: bool, out: &mut String) {
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
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ascii && (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------------------------------------------
// equality as jsonschema judges it
// ---------------------------------------------------------------------------------------------------------------

/// `jsonschema._utils.equal`: strings by text, lists and dicts item by item, `True` never equal to `1`, numbers by
/// value (`1 == 1.0`).
pub fn schema_equal(a: &Py, b: &Py) -> bool {
    match (a, b) {
        (Py::Str(x), Py::Str(y)) => x == y,
        (Py::Str(_), _) | (_, Py::Str(_)) => false,
        (Py::List(x), Py::List(y)) => {
            x.0.len() == y.0.len() && x.0.iter().zip(&y.0).all(|(p, q)| schema_equal(p, q))
        }
        (Py::Dict(x), Py::Dict(y)) => {
            x.0.len() == y.0.len()
                && x.0
                    .iter()
                    .all(|(k, v)| y.0.get(k).is_some_and(|w| schema_equal(v, w)))
        }
        (Py::Bool(x), Py::Bool(y)) => x == y,
        (Py::Bool(_), _) | (_, Py::Bool(_)) => false,
        (Py::None, Py::None) => true,
        (Py::None, _) | (_, Py::None) => false,
        (Py::Int(x), Py::Int(y)) => x == y,
        (Py::Float(x), Py::Float(y)) => x == y,
        (Py::Int(i), Py::Float(f)) | (Py::Float(f), Py::Int(i)) => int_equals_float(i, *f),
        _ => false,
    }
}

/// Python's exact `int == float`: false for NaN and the infinities, and for a float with a fraction.
fn int_equals_float(digits: &str, f: f64) -> bool {
    if !f.is_finite() || f.fract() != 0.0 {
        return false;
    }
    // A float with no fraction prints exactly as an integer (format! writes every digit of an integral f64).
    let whole = format!("{f:.0}");
    let whole = if whole == "-0" {
        "0".to_string()
    } else {
        whole
    };
    whole == digits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_values_read_write_clone_and_drop_without_recursion() {
        let text = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        let value = loads(&text).expect("deep but valid");
        assert_eq!(dumps(&value, Dumps::DEFAULT), text);
        assert_eq!(repr(&value.clone()), text);
        let bad = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH - 1));
        assert_eq!(
            loads(&bad),
            Err(LoadError::Value("Expecting ',' delimiter".into()))
        );
        let deeper = "[".repeat(200_000);
        assert_eq!(loads(&deeper), Err(LoadError::TooDeep));
    }

    #[test]
    fn numbers_keep_python_s_kinds() {
        let v =
            loads("[1, -0, 1.0, 1e400, -1e400, NaN, 123456789012345678901234567890, 1E2]").unwrap();
        assert_eq!(
            repr(&v),
            "[1, 0, 1.0, inf, -inf, nan, 123456789012345678901234567890, 100.0]"
        );
        assert_eq!(
            dumps(&v, Dumps::DEFAULT),
            "[1, 0, 1.0, Infinity, -Infinity, NaN, 123456789012345678901234567890, 100.0]"
        );
    }
}
