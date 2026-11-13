//! A call's arguments as `Gateway._build` reads them (`gateway.py:1134-1150`): the agent's notes taken out (`why`,
//! `effect`, and a generic input's declared `destination`), the rest validated by the tool's model, the destination
//! by `Destination` (W2: what a generic input declared it reaches, never a permission). An invalid call gets
//! `{"error": "invalid arguments", "details": [...]}` with pydantic's error structure and text.

use serde_json::{json, Map, Value};

use crate::pydantic::{Detail, Loc, Model};
use crate::tool::{Args, Tool};

/// What a generic input declared it reaches (`Destination`, `gateway.py:250`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Destination {
    pub recipients: Vec<String>,
    pub account: Option<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub item: Option<String>,
}

impl Destination {
    /// `model_dump(exclude_defaults=True)`.
    pub fn dump_set(&self) -> Map<String, Value> {
        let mut m = Map::new();
        if !self.recipients.is_empty() {
            m.insert("recipients".into(), json!(self.recipients));
        }
        if let Some(a) = &self.account {
            m.insert("account".into(), json!(a));
        }
        if let Some(a) = self.amount {
            m.insert("amount".into(), json!(a));
        }
        if let Some(c) = &self.currency {
            m.insert("currency".into(), json!(c));
        }
        if let Some(i) = &self.item {
            m.insert("item".into(), json!(i));
        }
        m
    }
}

/// `Destination`'s schema as pydantic validates it.
fn destination_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "recipients": {"items": {"type": "string"}, "type": "array"},
            "account": {"anyOf": [{"type": "string"}, {"type": "null"}], "default": null},
            "amount": {"anyOf": [{"minimum": 0, "type": "number"}, {"type": "null"}], "default": null},
            "currency": {"anyOf": [{"maxLength": 8, "type": "string"}, {"type": "null"}], "default": null},
            "item": {"anyOf": [{"type": "string"}, {"type": "null"}], "default": null}
        },
        "title": "Destination",
        "type": "object"
    })
}

/// The call's arguments, checked.
#[derive(Debug, Clone)]
pub struct Checked {
    /// The validated arguments (`model_dump(mode="json")`).
    pub args: Args,
    /// As given, without the agent's notes.
    pub arguments: Map<String, Value>,
    pub why: String,
    pub effect: String,
    /// The declared destination as given (None: not a generic input, or none given).
    pub noted: Option<Value>,
    pub reaches: Destination,
}

/// `str(value)` of a note as Python's `str(x or "")` reads it.
fn note_text(v: Option<Value>) -> String {
    match v {
        None => String::new(),
        Some(v) if !ferb_py::value::truthy(&v) => String::new(),
        Some(Value::String(s)) => s,
        Some(other) => ferb_py::value::py_str(&other),
    }
}

fn invalid(details: Vec<Detail>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("error".into(), Value::String("invalid arguments".into()));
    m.insert(
        "details".into(),
        Value::Array(details.iter().map(Detail::to_json).collect()),
    );
    m
}

/// `_build`'s first part: the notes, then the tool's model, then the destination. `Err`: the reply's content.
pub fn check(
    tool: &dyn Tool,
    arguments: &Map<String, Value>,
) -> Result<Checked, Map<String, Value>> {
    let mut arguments = arguments.clone();
    let why = ferb_py::value::strip(&note_text(arguments.shift_remove("why"))).to_string();
    let effect = ferb_py::value::strip(&note_text(arguments.shift_remove("effect"))).to_lowercase();
    let schema = tool.input_schema();
    let has_destination = schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key("destination"));
    let generic = tool.facts().generic_input && !has_destination;
    let noted = if generic {
        arguments.shift_remove("destination")
    } else {
        None
    };
    let hooks = tool.hooks();
    let mut model = Model::of(&schema);
    model.hooks = hooks.as_deref();
    let input = Value::Object(arguments.clone());
    let args = model.validate(&input).map_err(invalid)?;
    let given = match &noted {
        Some(v) if ferb_py::value::truthy(v) => v.clone(),
        _ => Value::Object(Map::new()),
    };
    let dschema = destination_schema();
    let reached = Model::of(&dschema).validate(&given).map_err(|errs| {
        invalid(
            errs.into_iter()
                .map(|e| e.under(&[Loc::Key("destination".into())]))
                .collect(),
        )
    })?;
    let text = |k: &str| reached.get(k).and_then(Value::as_str).map(str::to_string);
    let reaches = Destination {
        recipients: reached
            .get("recipients")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        account: text("account"),
        amount: reached.get("amount").and_then(Value::as_f64),
        currency: text("currency"),
        item: text("item"),
    };
    Ok(Checked {
        args,
        arguments,
        why,
        effect,
        noted,
        reaches,
    })
}
