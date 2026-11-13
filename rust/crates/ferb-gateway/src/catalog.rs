//! The tool catalog the models receive (ENG-3): every tool's input schema and description exactly as the Python build
//! serves them, read from the frozen `rust/fixtures/catalog/tools.json` (made by running `Gateway.tools_for` on the
//! Python daemon), never regenerated. pydantic writes those schemas; Rust can't regenerate their bytes, so the frozen
//! data is the registry's data.
//!
//! What the gateway adds to a tool's own schema (`gateway.py:717-733`): `effect` for a tool that acts, `destination`
//! for a generic input with no field of that name, and `why`, in that order, at the end of `properties`.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// The frozen catalog (key order kept: `serde_json` with `preserve_order`).
const TOOLS_JSON: &str = include_str!("../../../fixtures/catalog/tools.json");

/// One tool of the frozen catalog, as its capability declared it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// Its description as listed (the dynamic ones in the form with no saved secret and a 20 GB limit).
    pub description: String,
    /// The capability's own `Input` schema: the listed schema without what the gateway adds.
    pub schema: Value,
    pub scopes: BTreeSet<String>,
    pub identities: BTreeSet<String>,
    /// Every attribute the gateway reads (`getattr`/`hasattr`); a method is `"<method>"`.
    pub facts: Map<String, Value>,
}

/// The frozen data: the tools in registry order and the gateway's constants.
#[derive(Debug)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub constants: Map<String, Value>,
    raw: Value,
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl Catalog {
    fn load() -> Catalog {
        let raw: Value = serde_json::from_str(TOOLS_JSON).expect("the frozen catalog is JSON");
        let mut entries = Vec::new();
        for t in raw["tools"].as_array().expect("tools is a list") {
            let mut schema = t["input_schema"].clone();
            let added = strings(&t["added_by_gateway"]);
            if let Some(Value::Object(props)) = schema.get_mut("properties") {
                for key in &added {
                    props.shift_remove(key);
                }
            }
            entries.push(Entry {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                schema,
                scopes: strings(&t["scopes"]),
                identities: strings(&t["identities"]),
                facts: t["facts"].as_object().cloned().unwrap_or_default(),
            });
        }
        let constants = raw["constants"].as_object().cloned().unwrap_or_default();
        Catalog {
            entries,
            constants,
            raw,
        }
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// A constant of `gateway.py` as frozen (`EFFECTS`, `EFFECT_FIELD`, `DESTINATION_SCHEMA`, `WHY_HELP`, ...).
    pub fn constant(&self, name: &str) -> &Value {
        self.constants.get(name).unwrap_or(&Value::Null)
    }

    /// The whole frozen file, for tests.
    pub fn raw(&self) -> &Value {
        &self.raw
    }
}

/// The catalog, read once.
pub fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(Catalog::load)
}

/// The `effect` property the gateway adds to an acting tool (`gateway.py:728`).
pub fn effect_property() -> Value {
    let c = catalog();
    let mut p = Map::new();
    p.insert("type".into(), Value::String("string".into()));
    p.insert("enum".into(), c.constant("EFFECTS").clone());
    p.insert("description".into(), c.constant("EFFECT_FIELD").clone());
    Value::Object(p)
}

/// `DESTINATION_SCHEMA` (`gateway.py:2866`).
pub fn destination_property() -> Value {
    catalog().constant("DESTINATION_SCHEMA").clone()
}

/// The `why` property (`gateway.py:731`).
pub fn why_property() -> Value {
    let mut p = Map::new();
    p.insert("type".into(), Value::String("string".into()));
    p.insert("description".into(), catalog().constant("WHY_HELP").clone());
    Value::Object(p)
}

/// `ferb/act/browser/tools.py:1272` `_accounts`: FERB's own saved secrets as three descriptions list them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSecret {
    pub key: String,
    pub account_hint: String,
    pub allowed_origins: Vec<String>,
}

/// `_accounts(driver)`.
pub fn accounts(secrets: &[SavedSecret]) -> String {
    if secrets.is_empty() {
        return "No FERB secrets are set up.".to_string();
    }
    let parts: Vec<String> = secrets
        .iter()
        .map(|s| {
            format!(
                "{} ({}; for {})",
                s.key,
                s.account_hint,
                s.allowed_origins.join(", ")
            )
        })
        .collect();
    format!("Available: {}", parts.join("; "))
}

/// The three descriptions that end with `_accounts` (`tools.py:1212,1240,1261`): their fixed words.
const ACCOUNT_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "ferb_browser.fill_secret",
        "Fill one of FERB's own passwords into a field (from the Keychain; you never see it). Refused unless the \
         field's page is one the secret is allowed for. ",
    ),
    (
        "ferb_browser.fill_otp",
        "Fill the explicitly selected opaque inbox candidate into its origin-bound field; the code stays sealed. ",
    ),
    (
        "ferb_browser.otp_candidates",
        "List every opaque numeric candidate in FERB's unread inbox with sender, time, account and message identity \
         (subject/body withheld). Judge identity and freshness yourself; ask the owner if multiple candidates remain \
         plausible or identity evidence is insufficient. Codes never reach the model. ",
    ),
];

/// What a listing reads when it is made (`tools.py:1146,1212,1240,1261`): FERB's saved secrets, and the downloads
/// folder's limit in decimal GB (`None`: the limits can't be read, which reads as 20).
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub secrets: Vec<SavedSecret>,
    pub downloads_total_bytes: Option<u64>,
}

/// Python's `f"{x:g}"` for a float.
fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let exp = x.abs().log10().floor() as i32;
    if !(-4..6).contains(&exp) {
        let s = format!("{:.5e}", x);
        // mantissa trimmed, exponent with sign and two digits
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let decimals = (5 - exp).max(0) as usize;
    let s = format!("{:.*}", decimals, x);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// A tool's description as listed now: the frozen words, with the two live parts composed as Python composes them.
pub fn description(name: &str, live: &Live) -> Option<String> {
    let e = catalog().entry(name)?;
    if let Some((_, fixed)) = ACCOUNT_DESCRIPTIONS.iter().find(|(n, _)| *n == name) {
        return Some(format!("{fixed}{}", accounts(&live.secrets)));
    }
    if name.ends_with(".downloads") && e.description.contains(" 20 GB") {
        let total = live
            .downloads_total_bytes
            .map(|b| fmt_g(b as f64 / 1e9))
            .unwrap_or_else(|| "20".into());
        return Some(e.description.replacen(" 20 GB", &format!(" {total} GB"), 1));
    }
    Some(e.description.clone())
}

#[cfg(test)]
mod tests {
    use super::fmt_g;

    #[test]
    fn g_format() {
        assert_eq!(fmt_g(20.0), "20");
        assert_eq!(fmt_g(1.5), "1.5");
        assert_eq!(fmt_g(0.25), "0.25");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_g(0.00001), "1e-05");
    }
}
