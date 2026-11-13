//! pydantic 2's lax ("python mode") validation of a tool's arguments, read from the tool's frozen JSON schema
//! (`rust/fixtures/catalog/tools.json`), with pydantic-core's error structure and wording: what
//! `cap.Input.model_validate(arguments)` accepts, what it makes of each value (the validated model as
//! `model_dump(mode="json")` gives it, defaults filled in, in field order), and `e.errors(include_url=False)` when it
//! refuses (`gateway.py:1142-1150`). Checked against `validation_errors.json` (627 cases through the real
//! `Gateway.call`).
//!
//! The schema says every constraint the argument models declare (types, `Literal`s, `ge`/`le`/`gt`, lengths, extra
//! fields forbidden, nested models). The models' own validators (`model_validator`, `field_validator`) are not in a
//! schema: they are in `validators.rs`, by tool.
//!
//! Limits (no FERB model hits them): an integer past 64 bits, `allow_inf_nan` (a JSON number is always finite; the
//! text "inf" is refused as not finite for every float field, which is what FERB's fields declare), and union members
//! other than `str`, `int`, `float`, `bool`, `list[...]` and a model.

use serde_json::{Map, Number, Value};

/// One part of an error's `loc`.
#[derive(Debug, Clone, PartialEq)]
pub enum Loc {
    Key(String),
    Index(usize),
}

impl Loc {
    fn json(&self) -> Value {
        match self {
            Loc::Key(k) => Value::String(k.clone()),
            Loc::Index(i) => Value::Number((*i as u64).into()),
        }
    }
}

/// One entry of `ValidationError.errors(include_url=False)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub kind: String,
    pub loc: Vec<Loc>,
    pub msg: String,
    pub input: Value,
    pub ctx: Option<Map<String, Value>>,
}

impl Detail {
    pub fn new(kind: &str, loc: &[Loc], msg: impl Into<String>, input: &Value) -> Detail {
        Detail {
            kind: kind.to_string(),
            loc: loc.to_vec(),
            msg: msg.into(),
            input: input.clone(),
            ctx: None,
        }
    }

    fn with_ctx(mut self, ctx: Map<String, Value>) -> Detail {
        self.ctx = Some(ctx);
        self
    }

    /// A `ValueError` raised by a validator: `Value error, <text>`, the error kept in `ctx` as its text.
    ///
    /// Fixed against Python (catalog README, "Found in Python"): pydantic keeps the `ValueError` object itself in
    /// `ctx.error`, so the gateway's reply can't be serialized (`json.dumps` raises `TypeError`); here it is the
    /// error's text, so the worker gets the refusal.
    pub fn value_error(loc: &[Loc], text: &str, input: &Value) -> Detail {
        let mut ctx = Map::new();
        ctx.insert("error".into(), Value::String(text.to_string()));
        Detail::new("value_error", loc, format!("Value error, {text}"), input).with_ctx(ctx)
    }

    /// `{"type", "loc", "msg", "input", "ctx"?}` in pydantic's key order.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String(self.kind.clone()));
        m.insert(
            "loc".into(),
            Value::Array(self.loc.iter().map(Loc::json).collect()),
        );
        m.insert("msg".into(), Value::String(self.msg.clone()));
        m.insert("input".into(), self.input.clone());
        if let Some(ctx) = &self.ctx {
            m.insert("ctx".into(), Value::Object(ctx.clone()));
        }
        Value::Object(m)
    }

    /// Python's `repr` of this error as `e.errors()` holds it (`loc` a tuple).
    pub fn py_repr(&self) -> String {
        let r = |v: &Value| ferb_sanitize::parity::repr(&ferb_sanitize::Py::from(v));
        let loc: Vec<String> = self.loc.iter().map(|l| r(&l.json())).collect();
        let loc = if loc.len() == 1 {
            format!("({},)", loc[0])
        } else {
            format!("({})", loc.join(", "))
        };
        let mut out = format!(
            "{{'type': {}, 'loc': {loc}, 'msg': {}, 'input': {}",
            r(&Value::String(self.kind.clone())),
            r(&Value::String(self.msg.clone())),
            r(&self.input)
        );
        if let Some(ctx) = &self.ctx {
            out += &format!(", 'ctx': {}", r(&Value::Object(ctx.clone())));
        }
        out + "}"
    }

    /// The same error under `prefix` (`gateway.py:1149`: a destination's errors).
    pub fn under(mut self, prefix: &[Loc]) -> Detail {
        let mut loc = prefix.to_vec();
        loc.append(&mut self.loc);
        self.loc = loc;
        self
    }
}

/// A model's schema with the definitions its `$ref`s name.
pub struct Model<'a> {
    pub schema: &'a Value,
    pub defs: &'a Map<String, Value>,
    pub class_name: &'a str,
    /// The models' own validators (`field_validator`, `model_validator(mode="after")`), by model class.
    pub hooks: Option<&'a dyn Hooks>,
}

/// A model's validators that a schema doesn't carry.
pub trait Hooks {
    /// A `field_validator` on `class.field`, given the field's valid value: `None` when there is none, else the
    /// value it keeps or the `ValueError`'s text.
    fn field(&self, class: &str, field: &str, value: &Value) -> Option<Result<Value, String>>;
    /// The model's `model_validator(mode="after")`, run once all its fields are valid (`input`: what it was given).
    fn after(
        &self,
        class: &str,
        fields: &mut Map<String, Value>,
        input: &Value,
    ) -> Result<(), String>;
}

static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();

impl<'a> Model<'a> {
    /// A tool's `Input` model (its `$defs` read from the schema itself).
    pub fn of(schema: &'a Value) -> Model<'a> {
        let defs = schema
            .get("$defs")
            .and_then(Value::as_object)
            .unwrap_or_else(|| EMPTY.get_or_init(Map::new));
        Model {
            schema,
            defs,
            class_name: schema
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Input"),
            hooks: None,
        }
    }

    /// `model_validate(input)`: the model's fields as `model_dump(mode="json")` gives them, or every error.
    pub fn validate(&self, input: &Value) -> Result<Map<String, Value>, Vec<Detail>> {
        let mut errors = Vec::new();
        let out = model(
            self.schema,
            self.defs,
            self.class_name,
            input,
            &[],
            &mut errors,
            self.hooks,
        );
        match out {
            Some(m) if errors.is_empty() => Ok(m),
            _ => Err(errors),
        }
    }
}

fn model(
    schema: &Value,
    defs: &Map<String, Value>,
    class_name: &str,
    input: &Value,
    loc: &[Loc],
    errors: &mut Vec<Detail>,
    hooks: Option<&dyn Hooks>,
) -> Option<Map<String, Value>> {
    let Value::Object(given) = input else {
        let mut ctx = Map::new();
        ctx.insert("class_name".into(), Value::String(class_name.to_string()));
        errors.push(
            Detail::new(
                "model_type",
                loc,
                format!("Input should be a valid dictionary or instance of {class_name}"),
                input,
            )
            .with_ctx(ctx),
        );
        return None;
    };
    let empty = Map::new();
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let before = errors.len();
    let mut out = Map::new();
    for (name, field) in props {
        let mut at = loc.to_vec();
        at.push(Loc::Key(name.clone()));
        match given.get(name) {
            Some(v) => {
                if let Some(ok) = value(field, defs, v, &at, errors, hooks) {
                    match hooks.and_then(|h| h.field(class_name, name, &ok)) {
                        None => {
                            out.insert(name.clone(), ok);
                        }
                        Some(Ok(changed)) => {
                            out.insert(name.clone(), changed);
                        }
                        Some(Err(text)) => errors.push(Detail::value_error(&at, &text, v)),
                    }
                }
            }
            None if required.contains(&name.as_str()) => {
                errors.push(Detail::new("missing", &at, "Field required", input));
            }
            None => {
                out.insert(name.clone(), default_of(field));
            }
        }
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        for (k, v) in given {
            if !props.contains_key(k) {
                let mut at = loc.to_vec();
                at.push(Loc::Key(k.clone()));
                errors.push(Detail::new(
                    "extra_forbidden",
                    &at,
                    "Extra inputs are not permitted",
                    v,
                ));
            }
        }
    }
    if errors.len() != before {
        return None;
    }
    if let Some(h) = hooks {
        if let Err(text) = h.after(class_name, &mut out, input) {
            errors.push(Detail::value_error(loc, &text, input));
            return None;
        }
    }
    Some(out)
}

/// A field's default as `model_dump` writes it; a `default_factory` (no `default` in the schema) is its type's empty
/// value.
fn default_of(field: &Value) -> Value {
    if let Some(d) = field.get("default") {
        return d.clone();
    }
    match field.get("type").and_then(Value::as_str) {
        Some("array") => Value::Array(Vec::new()),
        Some("object") => Value::Object(Map::new()),
        _ => Value::Null,
    }
}

fn value(
    field: &Value,
    defs: &Map<String, Value>,
    v: &Value,
    loc: &[Loc],
    errors: &mut Vec<Detail>,
    hooks: Option<&dyn Hooks>,
) -> Option<Value> {
    if let Some(r) = field.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or(r);
        let def = defs.get(name)?;
        let class = def.get("title").and_then(Value::as_str).unwrap_or(name);
        return model(def, defs, class, v, loc, errors, hooks).map(Value::Object);
    }
    if let Some(Value::Array(members)) = field.get("anyOf") {
        let null = Value::Object(
            [("type".to_string(), Value::String("null".into()))]
                .into_iter()
                .collect(),
        );
        let nullable = members.contains(&null);
        let others: Vec<&Value> = members.iter().filter(|m| **m != null).collect();
        if v.is_null() && nullable {
            return Some(Value::Null);
        }
        if others.len() == 1 {
            return value(others[0], defs, v, loc, errors, hooks);
        }
        // smart union: the first member that takes it; else every member's errors under its tag
        let mut all = Vec::new();
        for m in &others {
            let mut errs = Vec::new();
            if let Some(ok) = value(m, defs, v, &[], &mut errs, hooks) {
                return Some(ok);
            }
            let tag = Loc::Key(tag_of(m, defs));
            all.extend(errs.into_iter().map(|e| {
                let mut at = loc.to_vec();
                at.push(tag.clone());
                e.under(&at)
            }));
        }
        errors.extend(all);
        return None;
    }
    if let Some(Value::Array(allowed)) = field.get("enum") {
        return literal(allowed, v, loc, errors);
    }
    if let Some(c) = field.get("const") {
        return literal(std::slice::from_ref(c), v, loc, errors);
    }
    match field.get("type").and_then(Value::as_str) {
        Some("string") => string(field, v, loc, errors),
        Some("integer") => integer(field, v, loc, errors),
        Some("number") => number(field, v, loc, errors),
        Some("boolean") => match lax_bool(v) {
            Ok(b) => Some(Value::Bool(b)),
            Err(kind) => {
                let msg = if kind == "bool_parsing" {
                    "Input should be a valid boolean, unable to interpret input"
                } else {
                    "Input should be a valid boolean"
                };
                errors.push(Detail::new(kind, loc, msg, v));
                None
            }
        },
        Some("array") => array(field, defs, v, loc, errors, hooks),
        Some("object") => match v {
            Value::Object(_) => Some(v.clone()),
            _ => {
                errors.push(Detail::new(
                    "dict_type",
                    loc,
                    "Input should be a valid dictionary",
                    v,
                ));
                None
            }
        },
        Some("null") => {
            if v.is_null() {
                Some(Value::Null)
            } else {
                errors.push(Detail::new("none_required", loc, "Input should be None", v));
                None
            }
        }
        _ => Some(v.clone()),
    }
}

/// How pydantic names a union member in an error's `loc`.
fn tag_of(m: &Value, defs: &Map<String, Value>) -> String {
    if let Some(r) = m.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or(r);
        return defs
            .get(name)
            .and_then(|d| d.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
    }
    match m.get("type").and_then(Value::as_str) {
        Some("string") => "str".into(),
        Some("integer") => "int".into(),
        Some("number") => "float".into(),
        Some("boolean") => "bool".into(),
        Some("array") => format!(
            "list[{}]",
            m.get("items")
                .map(|i| tag_of(i, defs))
                .unwrap_or_else(|| "any".into())
        ),
        _ => "any".into(),
    }
}

/// Python's `repr` of a literal value, as pydantic writes the expected ones.
fn literal_repr(v: &Value) -> String {
    match v {
        Value::String(s) => crate::words::str_repr(s),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

fn literal(allowed: &[Value], v: &Value, loc: &[Loc], errors: &mut Vec<Detail>) -> Option<Value> {
    if allowed.contains(v) {
        return Some(v.clone());
    }
    let reprs: Vec<String> = allowed.iter().map(literal_repr).collect();
    let expected = match reprs.len() {
        0 => String::new(),
        1 => reprs[0].clone(),
        n => format!("{} or {}", reprs[..n - 1].join(", "), reprs[n - 1]),
    };
    let mut ctx = Map::new();
    ctx.insert("expected".into(), Value::String(expected.clone()));
    errors.push(
        Detail::new(
            "literal_error",
            loc,
            format!("Input should be {expected}"),
            v,
        )
        .with_ctx(ctx),
    );
    None
}

fn plural(n: u64, one: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {one}s")
    }
}

fn length_ctx(key: &str, n: u64) -> Map<String, Value> {
    let mut ctx = Map::new();
    ctx.insert(key.into(), Value::Number(n.into()));
    ctx
}

fn string(field: &Value, v: &Value, loc: &[Loc], errors: &mut Vec<Detail>) -> Option<Value> {
    let Value::String(s) = v else {
        errors.push(Detail::new(
            "string_type",
            loc,
            "Input should be a valid string",
            v,
        ));
        return None;
    };
    let n = s.chars().count() as u64;
    if let Some(min) = field.get("minLength").and_then(Value::as_u64) {
        if n < min {
            errors.push(
                Detail::new(
                    "string_too_short",
                    loc,
                    format!("String should have at least {}", plural(min, "character")),
                    v,
                )
                .with_ctx(length_ctx("min_length", min)),
            );
            return None;
        }
    }
    if let Some(max) = field.get("maxLength").and_then(Value::as_u64) {
        if n > max {
            errors.push(
                Detail::new(
                    "string_too_long",
                    loc,
                    format!("String should have at most {}", plural(max, "character")),
                    v,
                )
                .with_ctx(length_ctx("max_length", max)),
            );
            return None;
        }
    }
    Some(v.clone())
}

/// pydantic-core's text to int: surrounding whitespace dropped, an optional sign, ASCII digits with single
/// underscores between them, and an optional fraction of zeros.
fn str_int(s: &str) -> Option<i64> {
    let s = s.trim();
    let whole = match s.split_once('.') {
        Some((whole, frac)) => {
            if !frac.bytes().all(|b| b == b'0') {
                return None;
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
        return None;
    }
    let mut value: i128 = 0;
    for b in digits.bytes() {
        match b {
            b'_' => {}
            b'0'..=b'9' => {
                value = value * 10 + i128::from(b - b'0');
                if value > i128::from(i64::MAX) + 1 {
                    return None;
                }
            }
            _ => return None,
        }
    }
    i64::try_from(if negative { -value } else { value }).ok()
}

/// What an `int` field makes of a value, or the error's type.
fn lax_int(v: &Value) -> Result<i64, &'static str> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                let f = n.as_f64().ok_or("int_type")?;
                if f.fract() != 0.0 {
                    Err("int_from_float")
                } else if f.abs() < 9.2e18 {
                    Ok(f as i64)
                } else {
                    Err("int_parsing_size")
                }
            }
        }
        Value::String(s) => str_int(s).ok_or("int_parsing"),
        _ => Err("int_type"),
    }
}

/// What a `float` field makes of a value, or the error's type.
fn lax_float(v: &Value) -> Result<f64, &'static str> {
    match v {
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64().ok_or("float_type"),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() || t.contains("__") || t.starts_with('_') || t.ends_with('_') {
                return Err("float_parsing");
            }
            t.replace('_', "")
                .parse::<f64>()
                .map_err(|_| "float_parsing")
        }
        _ => Err("float_type"),
    }
}

/// What a `bool` field makes of a value, or the error's type.
pub fn lax_bool(v: &Value) -> Result<bool, &'static str> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            if f == 0.0 {
                Ok(false)
            } else if f == 1.0 {
                Ok(true)
            } else {
                Err("bool_parsing")
            }
        }
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" if s.is_ascii() => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" if s.is_ascii() => Ok(true),
            _ => Err("bool_parsing"),
        },
        _ => Err("bool_type"),
    }
}

/// A bound as the message writes it: an `int` field's as the schema has it, a `float` field's as pydantic-core's
/// float display (`2.0` reads "2").
fn bound_text(bound: &Value, float: bool) -> String {
    if float {
        let f = bound.as_f64().unwrap_or(0.0);
        format!("{f}")
    } else {
        bound.to_string()
    }
}

fn bound_ctx(key: &str, bound: &Value, float: bool) -> Map<String, Value> {
    let mut ctx = Map::new();
    let v = if float {
        Number::from_f64(bound.as_f64().unwrap_or(0.0))
            .map(Value::Number)
            .unwrap_or(Value::Null)
    } else {
        bound.clone()
    };
    ctx.insert(key.into(), v);
    ctx
}

/// `ge`, `gt`, `le`, `lt`, in pydantic's order of checks.
fn bounds(field: &Value, x: f64, v: &Value, loc: &[Loc], float: bool) -> Option<Detail> {
    type Check<'s> = (&'s str, &'s str, &'s str, &'s str, fn(f64, f64) -> bool);
    let checks: [Check<'_>; 4] = [
        (
            "exclusiveMaximum",
            "lt",
            "less_than",
            "less than",
            |x, b| x < b,
        ),
        (
            "maximum",
            "le",
            "less_than_equal",
            "less than or equal to",
            |x, b| x <= b,
        ),
        (
            "exclusiveMinimum",
            "gt",
            "greater_than",
            "greater than",
            |x, b| x > b,
        ),
        (
            "minimum",
            "ge",
            "greater_than_equal",
            "greater than or equal to",
            |x, b| x >= b,
        ),
    ];
    // pydantic-core checks in the order the constraints are declared on the type: le/lt before ge/gt for its
    // ints and floats alike (`constrained_int`: multiple_of, le, lt, ge, gt).
    for (schema_key, ctx_key, kind, words, ok) in checks {
        if let Some(b) = field.get(schema_key) {
            let bound = b.as_f64().unwrap_or(0.0);
            if !ok(x, bound) {
                return Some(
                    Detail::new(
                        kind,
                        loc,
                        format!("Input should be {words} {}", bound_text(b, float)),
                        v,
                    )
                    .with_ctx(bound_ctx(ctx_key, b, float)),
                );
            }
        }
    }
    None
}

fn integer(field: &Value, v: &Value, loc: &[Loc], errors: &mut Vec<Detail>) -> Option<Value> {
    match lax_int(v) {
        Ok(i) => {
            if let Some(e) = bounds(field, i as f64, v, loc, false) {
                errors.push(e);
                return None;
            }
            Some(Value::Number(i.into()))
        }
        Err(kind) => {
            let msg = match kind {
                "int_parsing" => {
                    "Input should be a valid integer, unable to parse string as an integer"
                }
                "int_from_float" => {
                    "Input should be a valid integer, got a number with a fractional part"
                }
                "int_parsing_size" => {
                    "Unable to parse input string as an integer, exceeded maximum size"
                }
                _ => "Input should be a valid integer",
            };
            errors.push(Detail::new(kind, loc, msg, v));
            None
        }
    }
}

fn number(field: &Value, v: &Value, loc: &[Loc], errors: &mut Vec<Detail>) -> Option<Value> {
    match lax_float(v) {
        Ok(f) if !f.is_finite() => {
            errors.push(Detail::new(
                "finite_number",
                loc,
                "Input should be a finite number",
                v,
            ));
            None
        }
        Ok(f) => {
            if let Some(e) = bounds(field, f, v, loc, true) {
                errors.push(e);
                return None;
            }
            Number::from_f64(f).map(Value::Number)
        }
        Err(kind) => {
            let msg = if kind == "float_parsing" {
                "Input should be a valid number, unable to parse string as a number"
            } else {
                "Input should be a valid number"
            };
            errors.push(Detail::new(kind, loc, msg, v));
            None
        }
    }
}

fn array(
    field: &Value,
    defs: &Map<String, Value>,
    v: &Value,
    loc: &[Loc],
    errors: &mut Vec<Detail>,
    hooks: Option<&dyn Hooks>,
) -> Option<Value> {
    let Value::Array(items) = v else {
        errors.push(Detail::new(
            "list_type",
            loc,
            "Input should be a valid list",
            v,
        ));
        return None;
    };
    let n = items.len() as u64;
    let length = |key: &str, bound: u64| {
        let mut ctx = Map::new();
        ctx.insert("field_type".into(), Value::String("List".into()));
        ctx.insert(key.into(), Value::Number(bound.into()));
        ctx.insert("actual_length".into(), Value::Number(n.into()));
        ctx
    };
    if let Some(max) = field.get("maxItems").and_then(Value::as_u64) {
        if n > max {
            errors.push(
                Detail::new(
                    "too_long",
                    loc,
                    format!(
                        "List should have at most {} after validation, not {n}",
                        plural(max, "item")
                    ),
                    v,
                )
                .with_ctx(length("max_length", max)),
            );
            return None;
        }
    }
    let before = errors.len();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let mut at = loc.to_vec();
        at.push(Loc::Index(i));
        let got = match field.get("items") {
            Some(schema) => value(schema, defs, item, &at, errors, hooks),
            None => Some(item.clone()),
        };
        if let Some(ok) = got {
            out.push(ok);
        }
    }
    if errors.len() != before {
        return None;
    }
    if let Some(min) = field.get("minItems").and_then(Value::as_u64) {
        if n < min {
            errors.push(
                Detail::new(
                    "too_short",
                    loc,
                    format!(
                        "List should have at least {} after validation, not {n}",
                        plural(min, "item")
                    ),
                    v,
                )
                .with_ctx(length("min_length", min)),
            );
            return None;
        }
    }
    Some(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn float_bounds_read_as_pydantic_writes_them() {
        let f = json!({"type": "number", "minimum": 2});
        let e = Model::of(&json!({"properties": {"x": f}, "title": "Input", "type": "object"}))
            .validate(&json!({"x": 1.5}))
            .unwrap_err();
        assert_eq!(e[0].msg, "Input should be greater than or equal to 2");
        assert_eq!(e[0].ctx.as_ref().unwrap()["ge"], json!(2.0));
    }
}
