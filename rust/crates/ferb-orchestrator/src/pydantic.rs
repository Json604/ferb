//! A small reproduction of pydantic 2.13.5 (pydantic-core 2.46.5) validation for the answer models, on the values
//! `json.loads` gives (`Py`): the lax coercions, defaults, `Literal`, `X | None`, the one smart union, `min_length`,
//! extra keys ignored, `mode="before"` and `mode="after"` model validators in pydantic's order, and the error items
//! with `str(ValidationError)` byte for byte, since that text goes back to the model (supervisor.py:1515, 2444).
//! Checked against pydantic's own answers in `rust/fixtures/orchestrator/models.json`.
//!
//! A validator that raises something other than a `ValueError` (a `TypeError` on a malformed legacy shape) escapes
//! pydantic in Python; here it is a `ValidationError` whose [`ValidationError::raised`] is set.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use ferb_context::pyvalue::{self, repr, Dict, List, Py};
use indexmap::IndexMap;

use crate::py;

/// One part of an error's location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loc {
    Key(String),
    Index(usize),
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Loc::Key(k) => f.write_str(k),
            Loc::Index(i) => write!(f, "{i}"),
        }
    }
}

/// One item of `e.errors()`: `type`, `loc`, `msg`, `input`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorItem {
    pub kind: String,
    pub loc: Vec<Loc>,
    pub msg: String,
    pub input: Py,
}

/// An exception a validator raised that pydantic doesn't turn into a validation error (Python: it propagates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raised {
    /// `TypeError` or `AttributeError`.
    pub exception: &'static str,
    pub message: String,
}

/// `pydantic.ValidationError` for one model, or the exception that escaped a validator.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    title: String,
    items: Vec<ErrorItem>,
    raised: Option<Raised>,
}

impl ValidationError {
    pub fn errors(&self) -> &[ErrorItem] {
        &self.items
    }

    pub fn error_count(&self) -> usize {
        self.items.len()
    }

    /// The model's name in the title line.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Set when a validator raised another exception: Python never made a `ValidationError` (callers that catch
    /// only `ValidationError`/`ValueError` let it through).
    pub fn raised(&self) -> Option<&Raised> {
        self.raised.as_ref()
    }
}

impl fmt::Display for ValidationError {
    /// `str(e)`; for an escaped exception, `str(exc)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(r) = &self.raised {
            return f.write_str(&r.message);
        }
        let n = self.items.len();
        write!(
            f,
            "{n} validation error{} for {}",
            if n == 1 { "" } else { "s" },
            self.title
        )?;
        for item in &self.items {
            if !item.loc.is_empty() {
                let loc: Vec<String> = item.loc.iter().map(|l| l.to_string()).collect();
                write!(f, "\n{}", loc.join("."))?;
            }
            write!(
                f,
                "\n  {} [type={}, input_value={}, input_type={}]\n    For further information visit https://errors.pydantic.dev/2.13/v/{}",
                item.msg,
                item.kind,
                truncated(&repr(&item.input)),
                type_name(&item.input),
                item.kind
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// pydantic-core's `write_truncated_to_limited_bytes(.., 50)`: past 50 bytes, the first 25 (cut back to a
/// character) and the last 24 (cut forward to one) around "...".
fn truncated(text: &str) -> String {
    if text.len() <= 50 {
        return text.to_string();
    }
    let mut head = 25;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - 24;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}...{}", &text[..head], &text[tail..])
}

/// `type(x).__name__`.
pub fn type_name(v: &Py) -> &'static str {
    match v {
        Py::None => "NoneType",
        Py::Bool(_) => "bool",
        Py::Int(_) => "int",
        Py::Float(_) => "float",
        Py::Str(_) => "str",
        Py::List(_) => "list",
        Py::Dict(_) => "dict",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------------------------------------------

pub(crate) enum T {
    Str(usize),
    Int,
    Float,
    Bool,
    Lit(&'static [&'static str]),
    List(Box<T>, usize),
    Opt(Box<T>),
    Union(Vec<(&'static str, T)>),
    Dict,
    Model(&'static str),
}

pub(crate) type Before = fn(Py) -> Result<Py, Raised>;
pub(crate) type After = fn(&mut IndexMap<String, Py>) -> Result<(), String>;

pub(crate) struct Field {
    pub name: &'static str,
    pub t: T,
    pub default: Option<fn() -> Py>,
}

pub(crate) struct ModelDef {
    pub name: &'static str,
    pub fields: Vec<Field>,
    /// In the order pydantic runs them (a class's later-defined before-validator first).
    pub befores: Vec<Before>,
    pub after: Option<After>,
}

fn registry() -> &'static HashMap<&'static str, ModelDef> {
    static R: OnceLock<HashMap<&'static str, ModelDef>> = OnceLock::new();
    R.get_or_init(|| {
        crate::validate::models()
            .into_iter()
            .map(|m| (m.name, m))
            .collect()
    })
}

struct Cx {
    json: bool,
    errors: Vec<ErrorItem>,
}

impl Cx {
    fn push(&mut self, kind: &str, loc: &[Loc], msg: impl Into<String>, input: &Py) {
        self.errors.push(ErrorItem {
            kind: kind.to_string(),
            loc: loc.to_vec(),
            msg: msg.into(),
            input: input.clone(),
        });
    }
}

/// `Model.model_validate(value)` (`json`: false) or the model part of `model_validate_json` (true).
pub(crate) fn validate_model(
    name: &'static str,
    input: &Py,
    json: bool,
) -> Result<Py, ValidationError> {
    let mut cx = Cx {
        json,
        errors: Vec::new(),
    };
    let mut loc = Vec::new();
    match model(name, input, &mut loc, &mut cx) {
        Err(raised) => Err(ValidationError {
            title: name.to_string(),
            items: Vec::new(),
            raised: Some(raised),
        }),
        Ok(Some(v)) if cx.errors.is_empty() => Ok(v),
        Ok(_) => Err(ValidationError {
            title: name.to_string(),
            items: cx.errors,
            raised: None,
        }),
    }
}

/// `Model.model_validate_json(text)`: jiter's parse (its `json_invalid` words), then the model.
pub(crate) fn validate_model_json(name: &'static str, text: &str) -> Result<Py, ValidationError> {
    match parse_json(text) {
        Ok(value) => validate_model(name, &value, true),
        Err(message) => Err(ValidationError {
            title: name.to_string(),
            items: vec![ErrorItem {
                kind: "json_invalid".into(),
                loc: Vec::new(),
                msg: format!("Invalid JSON: {message}"),
                input: Py::Str(text.to_string()),
            }],
            raised: None,
        }),
    }
}

/// The text read as jiter (pydantic's parser) reads it: `NaN`, `Infinity` and `-Infinity` allowed, a lone surrogate
/// escape refused. Its error words are serde_json's, which jiter keeps, with jiter's own positions where they differ
/// (a misspelt `NaN`/`Infinity`, a `\u` escape with a character that isn't hex).
fn parse_json(text: &str) -> Result<Py, String> {
    let scan = scan_json(text);
    match serde_json::from_str::<serde_json::Value>(&scan.checked) {
        Ok(_) => {}
        Err(e) if e.to_string().starts_with("number out of range") => {} // jiter reads it as ±inf
        Err(e) => {
            let message = e.to_string();
            if let Some((at, wrong)) = scan.cut {
                // the text was cut where a misspelt word starts: serde's EOF there is jiter's error at its first
                // wrong character
                if message.starts_with("EOF while parsing")
                    && (e.line(), e.column()) == position(text, at)
                {
                    let (line, column) = position(text, wrong + 1);
                    return Err(format!("expected ident at line {line} column {column}"));
                }
            }
            if let Some(bad) = scan.bad_hex {
                let here = (e.line(), e.column());
                if message.starts_with("invalid escape")
                    && position(text, bad + 1) <= here
                    && here <= position(text, bad + 5)
                {
                    let (line, column) = position(text, bad + 1);
                    return Err(format!("invalid escape at line {line} column {column}"));
                }
            }
            return Err(message);
        }
    }
    pyvalue::loads(text).map_err(|e| format!("{e:?}"))
}

/// serde_json's (line, column) for the byte offset `end` (the column counts the bytes of the line up to it).
fn position(text: &str, end: usize) -> (usize, usize) {
    let before = &text.as_bytes()[..end.min(text.len())];
    let line = before.iter().filter(|b| **b == b'\n').count() + 1;
    let start = before
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |p| p + 1);
    (line, before.len() - start)
}

struct Scan {
    /// The text for serde: non-finite words as numbers of the same length, cut at a misspelt one.
    checked: String,
    /// A misspelt non-finite word: where it starts (the text is cut there) and its first wrong character.
    cut: Option<(usize, usize)>,
    /// The first character of a `\u` escape that isn't hex.
    bad_hex: Option<usize>,
}

fn scan_json(text: &str) -> Scan {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_str, mut esc) = (0, false, false);
    let mut bad_hex = None;
    let glued =
        |c: Option<&u8>| c.is_some_and(|c| c.is_ascii_alphanumeric() || b"._+-".contains(c));
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
                if c == b'u' && bad_hex.is_none() {
                    bad_hex = (1..=4)
                        .map(|k| i + k)
                        .find(|&k| !b.get(k).is_some_and(u8::is_ascii_hexdigit));
                }
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if (c == b'N' || c == b'I' || (c == b'-' && b.get(i + 1) == Some(&b'I')))
            && !glued(i.checked_sub(1).and_then(|p| b.get(p)))
        {
            let (word, same) = match c {
                b'N' => ("NaN", "0.0"),
                b'I' => ("Infinity", "1.000000"),
                _ => ("-Infinity", "-1.000000"),
            };
            let matched = word
                .bytes()
                .zip(b[i..].iter())
                .take_while(|(w, t)| w == *t)
                .count();
            if matched == word.len() {
                out.push_str(same);
                i += word.len();
                continue;
            }
            return Scan {
                checked: out,
                cut: Some((i, i + matched)),
                bad_hex,
            };
        }
        let ch = text[i..].chars().next().expect("in range");
        out.push(ch);
        i += ch.len_utf8();
    }
    Scan {
        checked: out,
        cut: None,
        bad_hex,
    }
}

fn model(
    name: &'static str,
    input: &Py,
    loc: &mut Vec<Loc>,
    cx: &mut Cx,
) -> Result<Option<Py>, Raised> {
    let def = &registry()[name];
    let mut data = input.clone();
    for before in &def.befores {
        data = before(data)?;
    }
    let Py::Dict(map) = &data else {
        let msg = if cx.json {
            "Input should be an object".to_string()
        } else {
            format!("Input should be a valid dictionary or instance of {name}")
        };
        cx.push("model_type", loc, msg, &data);
        return Ok(None);
    };
    let before = cx.errors.len();
    let mut out: IndexMap<String, Py> = IndexMap::new();
    for field in &def.fields {
        match map.0.get(field.name) {
            Some(v) => {
                loc.push(Loc::Key(field.name.to_string()));
                let got = value(&field.t, v, loc, cx)?;
                loc.pop();
                if let Some(v) = got {
                    out.insert(field.name.to_string(), v);
                }
            }
            None => match field.default {
                Some(d) => {
                    out.insert(field.name.to_string(), d());
                }
                None => {
                    loc.push(Loc::Key(field.name.to_string()));
                    cx.push("missing", loc, "Field required", &data);
                    loc.pop();
                }
            },
        }
    }
    if cx.errors.len() > before {
        return Ok(None);
    }
    if let Some(after) = def.after {
        if let Err(message) = after(&mut out) {
            cx.push("value_error", loc, format!("Value error, {message}"), input);
            return Ok(None);
        }
    }
    Ok(Some(Py::Dict(Dict(out))))
}

fn value(t: &T, v: &Py, loc: &mut Vec<Loc>, cx: &mut Cx) -> Result<Option<Py>, Raised> {
    match t {
        T::Str(min) => Ok(match v {
            Py::Str(s) if py::len(s) >= *min => Some(v.clone()),
            Py::Str(_) => {
                let s = if *min == 1 { "" } else { "s" };
                cx.push(
                    "string_too_short",
                    loc,
                    format!("String should have at least {min} character{s}"),
                    v,
                );
                None
            }
            _ => {
                cx.push("string_type", loc, "Input should be a valid string", v);
                None
            }
        }),
        T::Int => Ok(match int(v) {
            Ok(i) => Some(Py::Int(i)),
            Err((kind, msg)) => {
                cx.push(kind, loc, msg, v);
                None
            }
        }),
        T::Float => Ok(match float(v) {
            Ok(f) => Some(Py::Float(f)),
            Err((kind, msg)) => {
                cx.push(kind, loc, msg, v);
                None
            }
        }),
        T::Bool => Ok(match boolean(v) {
            Ok(b) => Some(Py::Bool(b)),
            Err((kind, msg)) => {
                cx.push(kind, loc, msg, v);
                None
            }
        }),
        T::Lit(allowed) => Ok(match v {
            Py::Str(s) if allowed.contains(&s.as_str()) => Some(v.clone()),
            _ => {
                cx.push(
                    "literal_error",
                    loc,
                    format!("Input should be {}", expected(allowed)),
                    v,
                );
                None
            }
        }),
        T::Dict => Ok(match v {
            Py::Dict(_) => Some(v.clone()),
            _ => {
                let msg = if cx.json {
                    "Input should be an object"
                } else {
                    "Input should be a valid dictionary"
                };
                cx.push("dict_type", loc, msg, v);
                None
            }
        }),
        T::Opt(inner) => match v {
            Py::None => Ok(Some(Py::None)),
            _ => value(inner, v, loc, cx),
        },
        T::List(item, min) => {
            let Py::List(items) = v else {
                let msg = if cx.json {
                    "Input should be a valid array"
                } else {
                    "Input should be a valid list"
                };
                cx.push("list_type", loc, msg, v);
                return Ok(None);
            };
            let before = cx.errors.len();
            let mut out = Vec::with_capacity(items.0.len());
            for (i, x) in items.0.iter().enumerate() {
                loc.push(Loc::Index(i));
                let got = value(item, x, loc, cx)?;
                loc.pop();
                if let Some(x) = got {
                    out.push(x);
                }
            }
            if cx.errors.len() > before {
                return Ok(None);
            }
            if out.len() < *min {
                let s = if *min == 1 { "" } else { "s" };
                cx.push(
                    "too_short",
                    loc,
                    format!(
                        "List should have at least {min} item{s} after validation, not {}",
                        out.len()
                    ),
                    v,
                );
                return Ok(None);
            }
            Ok(Some(Py::List(List(out))))
        }
        T::Union(members) => {
            let mut failed = Vec::new();
            for (label, member) in members {
                let mut inner = Cx {
                    json: cx.json,
                    errors: Vec::new(),
                };
                loc.push(Loc::Key(label.to_string()));
                let got = value(member, v, loc, &mut inner)?;
                loc.pop();
                if let Some(x) = got {
                    if inner.errors.is_empty() {
                        return Ok(Some(x));
                    }
                }
                failed.extend(inner.errors);
            }
            cx.errors.extend(failed);
            Ok(None)
        }
        T::Model(name) => model(name, v, loc, cx),
    }
}

fn expected(allowed: &[&str]) -> String {
    let shown: Vec<String> = allowed.iter().map(|a| pyvalue::str_repr(a)).collect();
    match shown.len() {
        0 => String::new(),
        1 => shown[0].clone(),
        n => format!("{} or {}", shown[..n - 1].join(", "), shown[n - 1]),
    }
}

type Coerced<T> = Result<T, (&'static str, &'static str)>;

const INT_TYPE: (&str, &str) = ("int_type", "Input should be a valid integer");
const INT_PARSING: (&str, &str) = (
    "int_parsing",
    "Input should be a valid integer, unable to parse string as an integer",
);
const INT_SIZE: (&str, &str) = (
    "int_parsing_size",
    "Unable to parse input string as an integer, exceeded maximum size",
);

/// `int` in lax mode.
fn int(v: &Py) -> Coerced<String> {
    match v {
        Py::Bool(b) => Ok(if *b { "1" } else { "0" }.into()),
        Py::Int(i) => Ok(i.clone()),
        Py::Float(f) => {
            if !f.is_finite() {
                return Err(("finite_number", "Input should be a finite number"));
            }
            if f.fract() != 0.0 {
                return Err((
                    "int_from_float",
                    "Input should be a valid integer, got a number with a fractional part",
                ));
            }
            const LIMIT: f64 = 9_223_372_036_854_775_808.0;
            if *f <= -LIMIT || *f >= LIMIT {
                return Err(INT_SIZE);
            }
            Ok((*f as i64).to_string())
        }
        Py::Str(s) => str_int(s),
        _ => Err(INT_TYPE),
    }
}

/// Text to int: surrounding whitespace dropped, a sign, ASCII digits with single underscores between them, an
/// optional fraction of zeros; past Python's 4,300 digits, too big.
fn str_int(s: &str) -> Coerced<String> {
    let s = py::strip(s);
    let whole = match s.split_once('.') {
        Some((whole, frac)) => {
            if frac.is_empty() || !frac.bytes().all(|b| b == b'0') {
                return Err(INT_PARSING);
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
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return Err(INT_PARSING);
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    if plain.len() > pyvalue::MAX_INT_DIGITS {
        return Err(INT_SIZE);
    }
    let trimmed = plain.trim_start_matches('0');
    if trimmed.is_empty() {
        return Ok("0".into());
    }
    Ok(if negative {
        format!("-{trimmed}")
    } else {
        trimmed.to_string()
    })
}

/// `float` in lax mode.
fn float(v: &Py) -> Coerced<f64> {
    const PARSING: (&str, &str) = (
        "float_parsing",
        "Input should be a valid number, unable to parse string as a number",
    );
    match v {
        Py::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Py::Int(i) => Ok(i.parse::<f64>().unwrap_or(f64::INFINITY)),
        Py::Float(f) => Ok(*f),
        Py::Str(s) => py_float(s).ok_or(PARSING),
        _ => Err(("float_type", "Input should be a valid number")),
    }
}

/// Python's `float(text)`: whitespace dropped, underscores only between digits, `inf`/`infinity`/`nan` in any case.
pub(crate) fn py_float(s: &str) -> Option<f64> {
    let s = py::strip(s);
    let (sign, body) = match s.as_bytes().first() {
        Some(b'-') => (-1.0, &s[1..]),
        Some(b'+') => (1.0, &s[1..]),
        _ => (1.0, s),
    };
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if lower == "nan" {
        return Some(f64::NAN);
    }
    let bytes = body.as_bytes();
    if bytes.is_empty()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || b"._eE+-".contains(b))
    {
        return None;
    }
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'_' {
            let before = i > 0 && bytes[i - 1].is_ascii_digit();
            let after = bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit());
            if !before || !after {
                return None;
            }
        }
    }
    let plain: String = body.chars().filter(|c| *c != '_').collect();
    if plain.starts_with(['+', '-']) {
        return None;
    }
    plain.parse::<f64>().ok().map(|f| sign * f)
}

/// `bool` in lax mode.
fn boolean(v: &Py) -> Coerced<bool> {
    const PARSING: (&str, &str) = (
        "bool_parsing",
        "Input should be a valid boolean, unable to interpret input",
    );
    const TYPE: (&str, &str) = ("bool_type", "Input should be a valid boolean");
    match v {
        Py::Bool(b) => Ok(*b),
        // an int inside 64 bits other than 0 or 1 can't be read; any other number isn't a boolean
        Py::Int(i) => match i.as_str() {
            "0" => Ok(false),
            "1" => Ok(true),
            _ if i.parse::<i64>().is_ok() => Err(PARSING),
            _ => Err(TYPE),
        },
        Py::Float(f) => {
            const LIMIT: f64 = 9_223_372_036_854_775_808.0;
            if *f == 0.0 {
                Ok(false)
            } else if *f == 1.0 {
                Ok(true)
            } else if f.is_finite() && f.fract() == 0.0 && *f > -LIMIT && *f < LIMIT {
                Err(PARSING)
            } else {
                Err(TYPE)
            }
        }
        Py::Str(s) => match s.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err(PARSING),
        },
        _ => Err(TYPE),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Python behaviour the before-validators rest on
// ---------------------------------------------------------------------------------------------------------------

/// Python truthiness.
pub(crate) fn truthy(v: &Py) -> bool {
    match v {
        Py::None => false,
        Py::Bool(b) => *b,
        Py::Int(i) => i != "0",
        Py::Float(f) => *f != 0.0,
        Py::Str(s) => !s.is_empty(),
        Py::List(l) => !l.0.is_empty(),
        Py::Dict(d) => !d.0.is_empty(),
    }
}

/// `a == b` for JSON values (1 == 1.0 == True).
pub(crate) fn py_eq(a: &Py, b: &Py) -> bool {
    fn num(v: &Py) -> Option<f64> {
        match v {
            Py::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Py::Int(i) => i.parse().ok(),
            Py::Float(f) => Some(*f),
            _ => None,
        }
    }
    match (a, b) {
        (Py::Int(x), Py::Int(y)) => x == y,
        (Py::None, Py::None) => true,
        (Py::Str(x), Py::Str(y)) => x == y,
        (Py::List(x), Py::List(y)) => {
            x.0.len() == y.0.len() && x.0.iter().zip(y.0.iter()).all(|(p, q)| py_eq(p, q))
        }
        (Py::Dict(x), Py::Dict(y)) => {
            x.0.len() == y.0.len()
                && x.0
                    .iter()
                    .all(|(k, v)| y.0.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

fn not_iterable(v: &Py) -> Raised {
    Raised {
        exception: "TypeError",
        message: format!("'{}' object is not iterable", type_name(v)),
    }
}

/// `iter(v)` as a list: a list's items, a string's characters, a dict's keys; anything else `TypeError`.
pub(crate) fn iterate(v: &Py) -> Result<Vec<Py>, Raised> {
    match v {
        Py::List(l) => Ok(l.0.clone()),
        Py::Str(s) => Ok(s.chars().map(|c| Py::Str(c.to_string())).collect()),
        Py::Dict(d) => Ok(d.0.keys().map(|k| Py::Str(k.clone())).collect()),
        _ => Err(not_iterable(v)),
    }
}

/// `[*v]`.
pub(crate) fn star(v: &Py) -> Result<Vec<Py>, Raised> {
    iterate(v).map_err(|_| Raised {
        exception: "TypeError",
        message: format!("Value after * must be an iterable, not {}", type_name(v)),
    })
}

/// `list(v or [])`.
pub(crate) fn list_or_empty(v: Option<&Py>) -> Result<Vec<Py>, Raised> {
    match v {
        Some(x) if truthy(x) => iterate(x),
        _ => Ok(Vec::new()),
    }
}

/// `list(dict.fromkeys(items))`: the first of equal items, in order; an unhashable item raises.
pub(crate) fn unique(items: Vec<Py>) -> Result<Vec<Py>, Raised> {
    let mut out: Vec<Py> = Vec::new();
    for item in items {
        if let Py::List(_) | Py::Dict(_) = item {
            return Err(Raised {
                exception: "TypeError",
                message: format!("unhashable type: '{}'", type_name(&item)),
            });
        }
        if !out.iter().any(|x| py_eq(x, &item)) {
            out.push(item);
        }
    }
    Ok(out)
}

/// `data.get(key)` on a dict, None otherwise.
pub(crate) fn get<'a>(data: &'a Py, key: &str) -> Option<&'a Py> {
    data.get(key)
}

pub(crate) fn is_str(v: Option<&Py>, s: &str) -> bool {
    matches!(v, Some(Py::Str(x)) if x == s)
}

// ---------------------------------------------------------------------------------------------------------------
// model_dump_json
// ---------------------------------------------------------------------------------------------------------------

/// pydantic's `model_dump_json()`: compact, raw UTF-8, NaN and the infinities as `null`, floats as Python writes
/// them but with the exponent unpadded (`1e-7`, `1e+16`).
pub(crate) fn dump_json(v: &Py) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

fn write_json(v: &Py, out: &mut String) {
    match v {
        Py::None => out.push_str("null"),
        Py::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Py::Int(i) => out.push_str(i),
        Py::Float(f) => {
            if f.is_finite() {
                out.push_str(&json_float(*f));
            } else {
                out.push_str("null");
            }
        }
        Py::Str(s) => write_json_str(s, out),
        Py::List(l) => {
            out.push('[');
            for (i, x) in l.0.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(x, out);
            }
            out.push(']');
        }
        Py::Dict(d) => {
            out.push('{');
            for (i, (k, x)) in d.0.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_str(k, out);
                out.push(':');
                write_json(x, out);
            }
            out.push('}');
        }
    }
}

fn json_float(f: f64) -> String {
    let text = pyvalue::float_repr(f);
    match text.split_once('e') {
        Some((m, e)) => {
            let (sign, digits) = e.split_at(1);
            format!("{m}e{sign}{}", digits.trim_start_matches('0'))
        }
        None => text,
    }
}

fn write_json_str(s: &str, out: &mut String) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_by_bytes_at_characters() {
        let s = format!("['{}']", "é".repeat(60));
        assert_eq!(
            truncated(&s),
            format!("['{}...{}']", "é".repeat(11), "é".repeat(11))
        );
    }

    #[test]
    fn floats_unpadded() {
        assert_eq!(json_float(1e-7), "1e-7");
        assert_eq!(json_float(1e16), "1e+16");
        assert_eq!(json_float(100000.0), "100000.0");
    }
}
