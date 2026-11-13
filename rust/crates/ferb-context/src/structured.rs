//! Checking a model's typed answer against the schema FERB asked for (ferb/core/structured.py, decision RUN-408 and
//! the owner's D32). Every runtime's final answer passes here once; the text of a misfit goes back to the model
//! (`shape::repair_note`), so the words are a contract: they are jsonschema 4.26.0's, chosen as its `best_match`
//! chooses, with Python's `repr` of the values inside, cut at 200 characters.
//!
//! The validator is Draft 2020-12 as `jsonschema.Draft202012Validator` runs it, for the keywords FERB's answer
//! schemas use (`type`, `enum`, `properties`, `required`, `additionalProperties`, `items`, `anyOf`, `minItems`, and
//! `allOf` and a local `$ref` into `$defs` for the tools' inlined forms; the
//! annotations `description`, `title` and the like are read and ignored as jsonschema ignores them). A schema with
//! any other keyword is refused when the validator is made (`Unsupported`), never checked loosely.

use serde_json::{Map, Value};

use crate::pyvalue::{loads, repr, schema_equal, str_repr, LoadError, Py, MAX_DEPTH};

/// What `check_final` refuses, in the words structured.py:22 and :26 raise (`ValueError` there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidAnswer(pub String);

impl std::fmt::Display for InvalidAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidAnswer {}

/// A schema keyword this validator does not implement: the schema is refused rather than checked loosely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub String);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this schema uses {}, which FERB's answer check doesn't implement",
            self.0
        )
    }
}

impl std::error::Error for Unsupported {}

/// Why `check_final` returned no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    /// The answer doesn't fit (structured.py's `ValueError`): its text goes to the model.
    Invalid(InvalidAnswer),
    /// FERB's own schema uses a keyword the port doesn't implement (Python would have checked it).
    Schema(Unsupported),
    /// The answer is nested past `pyvalue::MAX_DEPTH`: Python raises `RecursionError` out of `check_final` (not a
    /// misfit told back to the model, a failed call).
    TooDeep,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Invalid(e) => e.fmt(f),
            CheckError::Schema(e) => e.fmt(f),
            CheckError::TooDeep => {
                write!(f, "the answer is nested more than {MAX_DEPTH} levels deep")
            }
        }
    }
}

impl std::error::Error for CheckError {}

/// structured.py:15 `check_final`: the answer unchanged when it is JSON matching `schema`; otherwise the first
/// problem, as `best_match` picks it. No schema (or an empty one) means free text.
pub fn check_final(text: &str, schema: Option<&Value>) -> Result<String, CheckError> {
    let Some(schema) = schema.filter(|s| truthy_schema(s)) else {
        return Ok(text.to_string());
    };
    let validator = Validator::new(schema).map_err(CheckError::Schema)?;
    let value = loads(text).map_err(|e| match e {
        LoadError::Value(msg) => {
            CheckError::Invalid(InvalidAnswer(format!("the answer is not JSON ({msg})")))
        }
        LoadError::TooDeep => CheckError::TooDeep,
    })?;
    match validator.best_match(&value) {
        None => Ok(text.to_string()),
        Some(error) => {
            let where_ = if error.absolute_path.is_empty() {
                "the top level".to_string()
            } else {
                error
                    .absolute_path
                    .iter()
                    .map(PathItem::to_string)
                    .collect::<Vec<_>>()
                    .join("/")
            };
            let message = ferb_py::value::prefix(&error.message, 200);
            Err(CheckError::Invalid(InvalidAnswer(format!(
                "the answer doesn't match the schema at {where_}: {message}"
            ))))
        }
    }
}

/// `if not schema`: None, `{}`, and the other falsy JSON values.
fn truthy_schema(schema: &Value) -> bool {
    match schema {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

/// One step of an error's place in the answer: a property name or a list index.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathItem {
    Key(String),
    Index(usize),
}

impl std::fmt::Display for PathItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathItem::Key(k) => f.write_str(k),
            PathItem::Index(i) => write!(f, "{i}"),
        }
    }
}

/// The error `best_match` picked: its message and its place from the top of the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub message: String,
    pub absolute_path: Vec<PathItem>,
}

/// A `jsonschema.ValidationError`, as much of it as `best_match` reads.
#[derive(Debug)]
struct VError {
    message: String,
    keyword: &'static str,
    /// `relative_path`: from the instance its parent error was raised on (from the top for a top-level error)
    path: Vec<PathItem>,
    context: Vec<VError>,
    /// `_matches_type()`: the instance has the type the error's schema names (false when it names none)
    matches_type: bool,
}

/// The annotations and other keywords `Draft202012Validator` reads without failing anything.
const IGNORED: &[&str] = &[
    "description",
    "title",
    "default",
    "examples",
    "$comment",
    "$schema",
    "$id",
    "$anchor",
    "deprecated",
    "readOnly",
    "writeOnly",
    "format",
];

/// `jsonschema.Draft202012Validator(schema)` for FERB's answer schemas.
pub struct Validator<'s> {
    schema: &'s Value,
}

impl<'s> Validator<'s> {
    /// Refuses a schema holding a validation keyword this port doesn't implement (anywhere in it).
    pub fn new(schema: &'s Value) -> Result<Validator<'s>, Unsupported> {
        supported(schema, schema)?;
        Ok(Validator { schema })
    }

    /// `best_match(iter_errors(instance))`, or None when the answer fits.
    pub fn best_match(&self, instance: &Py) -> Option<Picked> {
        let errors = iter_errors(self.schema, self.schema, instance);
        let mut best: Option<&VError> = None;
        for error in &errors {
            // max(): the first of the errors with the largest key
            if best.is_none_or(|b| relevance_cmp(error, b) == std::cmp::Ordering::Greater) {
                best = Some(error);
            }
        }
        let mut best = best?;
        let mut absolute = best.path.clone();
        while !best.context.is_empty() {
            // heapq.nsmallest(2, context, key): stable, the first of equal keys first
            let mut first: Option<&VError> = None;
            for error in &best.context {
                if first.is_none_or(|f| relevance_cmp(error, f) == std::cmp::Ordering::Less) {
                    first = Some(error);
                }
            }
            let first = first.expect("context is not empty");
            let second = best
                .context
                .iter()
                .filter(|e| !std::ptr::eq(*e, first))
                .reduce(|a, b| {
                    if relevance_cmp(b, a) == std::cmp::Ordering::Less {
                        b
                    } else {
                        a
                    }
                });
            if second.is_some_and(|s| relevance_cmp(first, s) == std::cmp::Ordering::Equal) {
                break;
            }
            absolute.extend(first.path.iter().cloned());
            best = first;
        }
        Some(Picked {
            message: best.message.clone(),
            absolute_path: absolute,
        })
    }

    /// Whether the answer fits (`is_valid`).
    pub fn is_valid(&self, instance: &Py) -> bool {
        iter_errors(self.schema, self.schema, instance).is_empty()
    }
}

/// A local `$ref` ("#" or "#/a/b", JSON pointer escapes `~0` and `~1`) resolved in the root schema.
fn pointer<'v>(root: &'v Value, reference: &str) -> Option<&'v Value> {
    let path = reference.strip_prefix('#')?;
    if path.is_empty() {
        return Some(root);
    }
    let mut at = root;
    for part in path.strip_prefix('/')?.split('/') {
        let part = part.replace("~1", "/").replace("~0", "~");
        at = match at {
            Value::Object(map) => map.get(&part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(at)
}

fn supported(root: &Value, schema: &Value) -> Result<(), Unsupported> {
    let Value::Object(map) = schema else {
        return match schema {
            Value::Bool(_) => Ok(()),
            _ => Err(Unsupported(format!("a schema that is {schema}"))),
        };
    };
    for (key, value) in map {
        match key.as_str() {
            "type" | "enum" | "required" | "minItems" => {}
            "properties" => {
                for sub in value.as_object().into_iter().flat_map(Map::values) {
                    supported(root, sub)?;
                }
            }
            "$defs" => {
                for sub in value.as_object().into_iter().flat_map(Map::values) {
                    supported(root, sub)?;
                }
            }
            "additionalProperties" | "items" => supported(root, value)?,
            "anyOf" | "allOf" => {
                for sub in value.as_array().into_iter().flatten() {
                    supported(root, sub)?;
                }
            }
            "$ref" => {
                let found = value.as_str().and_then(|r| pointer(root, r));
                if found.is_none() {
                    return Err(Unsupported(format!("the reference {value}")));
                }
            }
            k if IGNORED.contains(&k) => {}
            other => return Err(Unsupported(format!("the keyword {other:?}"))),
        }
    }
    Ok(())
}

/// The relevance key of exceptions.py:415: `(-len(path), path, validator not in WEAK, validator in STRONG,
/// not _matches_type())`, compared as Python compares tuples. STRONG is empty.
fn relevance_cmp(a: &VError, b: &VError) -> std::cmp::Ordering {
    let weak = |e: &VError| matches!(e.keyword, "anyOf" | "oneOf");
    b.path
        .len()
        .cmp(&a.path.len())
        .then_with(|| a.path.cmp(&b.path))
        .then_with(|| (!weak(a)).cmp(&!weak(b)))
        .then_with(|| (!a.matches_type).cmp(&!b.matches_type))
}

/// `TypeChecker.is_type` of Draft 2020-12: an integer is an int or a float with no fraction; a bool is neither a
/// number nor an integer. An unknown type name matches nothing (jsonschema raises `UnknownType`; FERB's schemas
/// have none).
fn is_type(instance: &Py, name: &str) -> bool {
    match name {
        "object" => matches!(instance, Py::Dict(_)),
        "array" => matches!(instance, Py::List(_)),
        "string" => matches!(instance, Py::Str(_)),
        "boolean" => matches!(instance, Py::Bool(_)),
        "null" => matches!(instance, Py::None),
        "number" => matches!(instance, Py::Int(_) | Py::Float(_)),
        "integer" => match instance {
            Py::Int(_) => true,
            Py::Float(f) => f.is_finite() && f.fract() == 0.0,
            _ => false,
        },
        _ => false,
    }
}

fn type_names(types: &Value) -> Vec<&str> {
    match types {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// `_Error._matches_type`: the error's schema names a type and the instance has it.
fn matches_type(schema: &Value, instance: &Py) -> bool {
    match schema.get("type") {
        None => false,
        Some(types) => type_names(types).into_iter().any(|t| is_type(instance, t)),
    }
}

/// `iter_errors` / `descend`: each keyword of the schema in its own order, each failing keyword's errors in order.
fn iter_errors(root: &Value, schema: &Value, instance: &Py) -> Vec<VError> {
    let mut out = Vec::new();
    let Value::Object(map) = schema else {
        if schema == &Value::Bool(false) {
            out.push(VError {
                message: format!("False schema does not allow {}", repr(instance)),
                keyword: "",
                path: Vec::new(),
                context: Vec::new(),
                matches_type: false,
            });
        }
        return out;
    };
    let fits = matches_type(schema, instance);
    let error = |message: String, keyword: &'static str| VError {
        message,
        keyword,
        path: Vec::new(),
        context: Vec::new(),
        matches_type: fits,
    };
    for (key, value) in map {
        match key.as_str() {
            "type" => {
                let types = type_names(value);
                if !types.iter().any(|t| is_type(instance, t)) {
                    let reprs = types
                        .iter()
                        .map(|t| str_repr(t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push(error(
                        format!("{} is not of type {reprs}", repr(instance)),
                        "type",
                    ));
                }
            }
            "enum" => {
                let options: Vec<Py> = value
                    .as_array()
                    .map(|items| items.iter().map(Py::from_value).collect())
                    .unwrap_or_default();
                if !options.iter().any(|o| schema_equal(o, instance)) {
                    out.push(error(
                        format!(
                            "{} is not one of {}",
                            repr(instance),
                            repr(&Py::from_value(value))
                        ),
                        "enum",
                    ));
                }
            }
            "properties" => {
                let (Py::Dict(dict), Value::Object(props)) = (instance, value) else {
                    continue;
                };
                for (name, sub) in props {
                    if let Some(item) = dict.0.get(name) {
                        for mut e in iter_errors(root, sub, item) {
                            e.path.insert(0, PathItem::Key(name.clone()));
                            out.push(e);
                        }
                    }
                }
            }
            "required" => {
                let Py::Dict(dict) = instance else { continue };
                for name in value.as_array().into_iter().flatten() {
                    if let Some(name) = name.as_str() {
                        if !dict.0.contains_key(name) {
                            out.push(error(
                                format!("{} is a required property", str_repr(name)),
                                "required",
                            ));
                        }
                    }
                }
            }
            "additionalProperties" => {
                let Py::Dict(dict) = instance else { continue };
                let props = map.get("properties").and_then(Value::as_object);
                let extras: Vec<&String> = dict
                    .0
                    .keys()
                    .filter(|k| !props.is_some_and(|p| p.contains_key(*k)))
                    .collect();
                match value {
                    Value::Object(_) => {
                        // jsonschema walks a set here (no fixed order); FERB's schemas only use `false`
                        for name in extras {
                            for mut e in iter_errors(root, value, &dict.0[name]) {
                                e.path.insert(0, PathItem::Key(name.clone()));
                                out.push(e);
                            }
                        }
                    }
                    Value::Bool(false) if !extras.is_empty() => {
                        let mut sorted = extras;
                        sorted.sort();
                        let verb = if sorted.len() == 1 { "was" } else { "were" };
                        let names = sorted
                            .iter()
                            .map(|k| str_repr(k))
                            .collect::<Vec<_>>()
                            .join(", ");
                        out.push(error(
                            format!(
                                "Additional properties are not allowed ({names} {verb} unexpected)"
                            ),
                            "additionalProperties",
                        ));
                    }
                    _ => {}
                }
            }
            "items" => {
                let Py::List(list) = instance else { continue };
                match value {
                    Value::Bool(false) if !list.0.is_empty() => {
                        let rest = if list.0.len() == 1 {
                            repr(&list.0[0])
                        } else {
                            repr(instance)
                        };
                        out.push(error(
                            format!(
                                "Expected at most 0 items but found {} extra: {rest}",
                                list.0.len()
                            ),
                            "items",
                        ));
                    }
                    _ => {
                        for (index, item) in list.0.iter().enumerate() {
                            for mut e in iter_errors(root, value, item) {
                                e.path.insert(0, PathItem::Index(index));
                                out.push(e);
                            }
                        }
                    }
                }
            }
            "minItems" => {
                let Py::List(list) = instance else { continue };
                let least = value.as_f64().unwrap_or(0.0);
                if (list.0.len() as f64) < least {
                    let says = if least == 1.0 {
                        "should be non-empty"
                    } else {
                        "is too short"
                    };
                    out.push(error(format!("{} {says}", repr(instance)), "minItems"));
                }
            }
            "allOf" => {
                for sub in value.as_array().into_iter().flatten() {
                    out.extend(iter_errors(root, sub, instance));
                }
            }
            "$ref" => {
                // checked by `supported`: a local pointer that resolves
                if let Some(target) = value.as_str().and_then(|r| pointer(root, r)) {
                    out.extend(iter_errors(root, target, instance));
                }
            }
            "anyOf" => {
                let mut all = Vec::new();
                let mut matched = false;
                for sub in value.as_array().into_iter().flatten() {
                    let errors = iter_errors(root, sub, instance);
                    if errors.is_empty() {
                        matched = true;
                        break;
                    }
                    all.extend(errors);
                }
                if !matched {
                    out.push(VError {
                        message: format!(
                            "{} is not valid under any of the given schemas",
                            repr(instance)
                        ),
                        keyword: "anyOf",
                        path: Vec::new(),
                        context: all,
                        matches_type: fits,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn problem(text: &str, schema: &Value) -> String {
        match check_final(text, Some(schema)) {
            Err(CheckError::Invalid(InvalidAnswer(m))) => m,
            other => panic!("expected a misfit, got {other:?}"),
        }
    }

    #[test]
    fn structured_results_tests_hold() {
        // tests/test_structured_results.py
        let schema = json!({"type": "object", "properties": {"ok": {"type": "boolean"}},
                            "required": ["ok"], "additionalProperties": false});
        assert_eq!(
            check_final("{\"ok\": true}", Some(&schema)).unwrap(),
            "{\"ok\": true}"
        );
        assert!(problem("not json", &schema).contains("not JSON"));
        assert!(problem("{\"ok\": \"yes\"}", &schema).contains("at ok"));
        assert!(problem("{}", &schema).contains("top level"));
        assert!(problem("{\"ok\": true, \"extra\": 1}", &schema).contains("top level"));
        assert_eq!(check_final("plain words", None).unwrap(), "plain words");
    }

    #[test]
    fn a_keyword_the_port_lacks_refuses_the_schema() {
        let schema = json!({"type": "string", "pattern": "^a"});
        assert!(matches!(
            check_final("\"b\"", Some(&schema)),
            Err(CheckError::Schema(_))
        ));
    }
}
