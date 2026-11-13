//! Reading the fixtures: rust/fixtures/supervisor (made by its make.py), rust/fixtures/catalog and
//! rust/fixtures/daemon. Texts of 2,048 characters or more are stored once by SHA-256 (`{"sha256"}`).
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;

use ferb_context::assemble::ContextBlock;
use ferb_context::prompts::PromptRoot;
use ferb_supervisor::history::Sourced;
use serde_json::Value;

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn read(rel: &str) -> Value {
    let path = repo().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

fn bodies() -> &'static HashMap<String, String> {
    static B: OnceLock<HashMap<String, String>> = OnceLock::new();
    B.get_or_init(|| {
        let mut out = HashMap::new();
        for rel in [
            "rust/fixtures/daemon/bodies.json",
            "rust/fixtures/supervisor/bodies.json",
        ] {
            if let Value::Object(map) = read(rel) {
                for (k, v) in map {
                    out.insert(k, v.as_str().expect("a body").to_string());
                }
            }
        }
        out
    })
}

/// A value with every `{"sha256": …}` stand-in replaced by its text.
pub fn resolve(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            let key = m["sha256"].as_str().expect("hash");
            Value::String(
                bodies()
                    .get(key)
                    .unwrap_or_else(|| panic!("no body {key}"))
                    .clone(),
            )
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), resolve(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(resolve).collect()),
        other => other.clone(),
    }
}

pub fn root() -> PromptRoot {
    PromptRoot::new(repo().join("ferb")).expect("the prompt package")
}

pub const WRAP_ID: &str = "0123456789ab";

pub fn wrap(text: &str) -> String {
    ferb_sanitize::untrusted::wrap_untrusted_with_id(text, WRAP_ID)
}

pub fn s(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

pub fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

pub fn set(v: &Value) -> BTreeSet<String> {
    strs(v).into_iter().collect()
}

/// A recorded block (make.py `block`).
pub fn block(v: &Value) -> ContextBlock {
    ContextBlock {
        text: s(&v["text"]),
        reference: s(&v["reference"]),
        protected: v["protected"].as_bool().unwrap_or(false),
        cache: v["cache"].as_bool().unwrap_or(false),
        priority: v["priority"].as_i64().unwrap_or(0),
        origin: s(&v["origin"]),
        source_refs: strs(&v["source_refs"]),
        payload: v["payload"].as_str().map(str::to_string),
        history: v["history"].as_bool().unwrap_or(false),
        fold: s(&v["fold"]),
        values: if v["values"].is_null() {
            None
        } else {
            Some(v["values"].clone())
        },
    }
}

/// A recorded (block, origins) pair (make.py `sourced`).
pub fn sourced(v: &Value) -> Sourced {
    Sourced {
        block: block(&v["block"]),
        origins: set(&v["origins"]),
    }
}

/// A catalog stored once by make.py (`{"catalog": sha}`).
pub fn catalog(v: &Value) -> Vec<Value> {
    static C: OnceLock<Value> = OnceLock::new();
    let all =
        C.get_or_init(|| resolve(&read("rust/fixtures/supervisor/scenarios.json")["catalogs"]));
    all[s(&v["catalog"])].as_array().expect("a catalog").clone()
}

/// The scenarios' records, bodies resolved.
pub fn scenarios() -> &'static Vec<(String, Vec<Value>)> {
    static S: OnceLock<Vec<(String, Vec<Value>)>> = OnceLock::new();
    S.get_or_init(|| {
        let all = read("rust/fixtures/supervisor/scenarios.json");
        all["scenarios"]
            .as_object()
            .expect("scenarios")
            .iter()
            .map(|(name, sc)| {
                (
                    name.clone(),
                    sc["records"]
                        .as_array()
                        .expect("records")
                        .iter()
                        .map(resolve)
                        .collect(),
                )
            })
            .collect()
    })
}
