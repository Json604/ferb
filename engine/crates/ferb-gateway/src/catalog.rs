//! The tool catalog the models receive (ENG-3): every tool's input schema and description exactly as the Python build
//! serves them, read from the frozen `engine/fixtures/catalog/tools.json` (made by running `Gateway.tools_for` on the
//! Python daemon), never regenerated. pydantic writes those schemas; Rust can't regenerate their bytes, so the frozen
//! data is the registry's data.
//!
//! The Rust engine's own tools beyond Python's (`catalog_additions.json`, each with the decision that adds it) join the
//! entries after the tool each names; [`Catalog::raw`] stays Python's file as frozen, and [`additions`] names them for
//! the tests that compare the two (decision: DMN-595, `about.diagnostics`).
//!
//! What the gateway adds to a tool's own schema (`gateway.py:717-733`): `effect` for a tool that acts, `destination`
//! for a generic input with no field of that name, and `why`, in that order, at the end of `properties`.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// The frozen catalog (key order kept: `serde_json` with `preserve_order`).
const TOOLS_JSON: &str = include_str!("../../../fixtures/catalog/tools.json");
/// The Rust engine's own tools (not Python's), each with its decision and the tool it follows.
const ADDITIONS_JSON: &str = include_str!("catalog_additions.json");
/// Each tool's one line, what it does and when to use it (ORCH-752): how a run lists a tool outside its full core.
const LINES_JSON: &str = include_str!("tool_lines.json");
/// The deliberate changes to Python's tools (`engine/fixtures/catalog/tool_changes.json`), each with its decision.
const CHANGES_JSON: &str = include_str!("../../../fixtures/catalog/tool_changes.json");

/// One deliberate change to a tool of Python's catalog (HAND-551 the first): `old` replaced by `new`, once, in the
/// tool's `description`, in its `input_schema` as `json.dumps` writes it (so in every listing's bytes), or
/// (`validation`) in the texts its argument checks write. Every test that compares with a recorded catalog text applies
/// them ([`apply_changes`]) or takes them out ([`revert_changes`]); every other byte stays Python's.
#[derive(Debug, Clone)]
pub struct Change {
    pub decision: String,
    pub tool: String,
    /// `description`, `input_schema` or `validation`.
    pub part: String,
    pub old: String,
    pub new: String,
}

fn load_changes() -> Vec<Change> {
    let v: Value = serde_json::from_str(CHANGES_JSON).expect("tool_changes.json is JSON");
    v["changes"]
        .as_array()
        .expect("changes is a list")
        .iter()
        .map(|c| {
            let s = |k: &str| c[k].as_str().expect(k).to_string();
            Change {
                decision: s("decision"),
                tool: s("tool"),
                part: s("part"),
                old: s("old"),
                new: s("new"),
            }
        })
        .collect()
}

/// The deliberate changes to Python's tools, in order.
pub fn changes() -> &'static [Change] {
    static C: OnceLock<Vec<Change>> = OnceLock::new();
    C.get_or_init(load_changes)
}

/// A text as it may sit in a recorded text: raw, JSON-escaped once and twice (both `json.dumps` forms).
fn forms(text: &str) -> Vec<String> {
    let mut out = vec![text.to_string()];
    for ascii in [true, false] {
        let esc = |t: &str| {
            let v = Value::String(t.to_string());
            let d = if ascii {
                ferb_py::json::dumps(&v)
            } else {
                ferb_py::json::dumps_utf8(&v)
            };
            d[1..d.len() - 1].to_string()
        };
        let once = esc(text);
        let twice = esc(&once);
        for f in [once, twice] {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

fn swap(text: &str, from: &str, to: &str) -> String {
    let mut out = text.to_string();
    for (f, t) in forms(from).into_iter().zip(forms(to)) {
        if out.contains(&f) {
            out = out.replace(&f, &t);
        }
    }
    out
}

/// A recorded text (a listing, a schema, a description, an argument check's reply) with every change applied, in
/// the raw and JSON-escaped forms.
pub fn apply_changes(text: &str) -> String {
    changes()
        .iter()
        .fold(text.to_string(), |t, c| swap(&t, &c.old, &c.new))
}

/// A recorded text of one tool (its row's schema, description or argument checks) with that tool's changes applied:
/// a change's old text may sit in other tools' rows too (ORCH-753's `whole` follows the `tab` every browser tool has).
pub fn apply_changes_for(tool: &str, text: &str) -> String {
    changes()
        .iter()
        .filter(|c| c.tool == tool)
        .fold(text.to_string(), |t, c| swap(&t, &c.old, &c.new))
}

/// A text of today's catalog with every change taken out: what Python wrote.
pub fn revert_changes(text: &str) -> String {
    changes()
        .iter()
        .rev()
        .fold(text.to_string(), |t, c| swap(&t, &c.new, &c.old))
}

/// How many bytes the changes add to a listing (`json.dumps`, ASCII) that offers these tools: the token reserve
/// counts them.
pub fn changes_shift(tools: &[String]) -> i64 {
    changes()
        .iter()
        .filter(|c| c.part != "validation" && tools.contains(&c.tool))
        .map(|c| {
            // a schema piece is JSON text inside the listing as it is; a description is a string, escaped
            let d = |t: &str| {
                if c.part == "input_schema" {
                    t.len() as i64
                } else {
                    ferb_py::json::dumps(&Value::String(t.to_string())).len() as i64
                }
            };
            d(&c.new) - d(&c.old)
        })
        .sum()
}

fn changed(e: &mut Entry) {
    for c in changes().iter().filter(|c| c.tool == e.name) {
        match c.part.as_str() {
            "description" => {
                assert!(e.description.contains(&c.old), "{}: {}", c.decision, c.tool);
                e.description = e.description.replacen(&c.old, &c.new, 1);
            }
            "input_schema" => {
                let text = ferb_py::json::dumps(&e.schema);
                assert!(text.contains(&c.old), "{}: {}", c.decision, c.tool);
                e.schema = serde_json::from_str(&text.replacen(&c.old, &c.new, 1))
                    .expect("a changed schema is JSON");
            }
            _ => {}
        }
    }
}

/// One tool the Rust engine adds to Python's catalog.
#[derive(Debug, Clone)]
pub struct Addition {
    /// The decision that adds it.
    pub decision: String,
    /// The Python tool it follows in registry order.
    pub after: String,
    pub name: String,
    /// Its row as written in `catalog_additions.json` (the same fields as a tools.json row).
    pub row: Value,
}

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
    /// ORCH-752: its one line, what it does and when to use it (`tool_lines.json`), as a run lists it outside its
    /// full core.
    pub line: String,
}

/// The frozen data: the tools in registry order and the gateway's constants.
#[derive(Debug)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    /// The Rust engine's own tools, also among `entries`.
    pub additions: Vec<Addition>,
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

fn entry_of(t: &Value) -> Entry {
    let mut schema = t["input_schema"].clone();
    let added = strings(&t["added_by_gateway"]);
    if let Some(Value::Object(props)) = schema.get_mut("properties") {
        for key in &added {
            props.shift_remove(key);
        }
    }
    Entry {
        name: t["name"].as_str().unwrap_or_default().to_string(),
        description: t["description"].as_str().unwrap_or_default().to_string(),
        schema,
        scopes: strings(&t["scopes"]),
        identities: strings(&t["identities"]),
        facts: t["facts"].as_object().cloned().unwrap_or_default(),
        line: String::new(),
    }
}

/// `tool_lines.json`'s lines, by tool (ORCH-752).
pub fn lines() -> &'static Map<String, Value> {
    static L: OnceLock<Map<String, Value>> = OnceLock::new();
    L.get_or_init(|| {
        let v: Value = serde_json::from_str(LINES_JSON).expect("tool_lines.json is JSON");
        v["lines"].as_object().cloned().expect("lines is an object")
    })
}

impl Catalog {
    fn load() -> Catalog {
        let raw: Value = serde_json::from_str(TOOLS_JSON).expect("the frozen catalog is JSON");
        let mut entries: Vec<Entry> = raw["tools"]
            .as_array()
            .expect("tools is a list")
            .iter()
            .map(entry_of)
            .collect();
        entries.iter_mut().for_each(changed);
        let extra: Value = serde_json::from_str(ADDITIONS_JSON).expect("the additions are JSON");
        let mut additions = Vec::new();
        for t in extra["tools"].as_array().expect("tools is a list") {
            let a = Addition {
                decision: t["decision"].as_str().unwrap_or_default().to_string(),
                after: t["after"].as_str().unwrap_or_default().to_string(),
                name: t["name"].as_str().unwrap_or_default().to_string(),
                row: t.clone(),
            };
            let mut at = entries
                .iter()
                .position(|e| e.name == a.after)
                .expect("an addition follows a catalog tool")
                + 1;
            // ORCH-736: several additions following one tool (the front door's six after chat.history) keep the
            // file's order
            while entries.get(at).is_some_and(|e| {
                additions
                    .iter()
                    .any(|b: &Addition| b.name == e.name && b.after == a.after)
            }) {
                at += 1;
            }
            entries.insert(at, entry_of(t));
            additions.push(a);
        }
        for e in &mut entries {
            if let Some(Value::String(l)) = lines().get(&e.name) {
                e.line = l.clone();
            }
        }
        let constants = raw["constants"].as_object().cloned().unwrap_or_default();
        Catalog {
            entries,
            additions,
            constants,
            raw,
        }
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// ORCH-752: a tool's one line ("" when none is written).
    pub fn line(&self, name: &str) -> &str {
        self.entry(name).map(|e| e.line.as_str()).unwrap_or("")
    }

    /// A constant of `gateway.py` as frozen (`EFFECTS`, `EFFECT_FIELD`, `DESTINATION_SCHEMA`, `WHY_HELP`, ...).
    pub fn constant(&self, name: &str) -> &Value {
        self.constants.get(name).unwrap_or(&Value::Null)
    }

    /// The whole frozen file (Python's, without the additions), for tests.
    pub fn raw(&self) -> &Value {
        &self.raw
    }
}

/// The Rust engine's own tools, in the order they were added.
pub fn additions() -> &'static [Addition] {
    &catalog().additions
}

/// Whether `name` is one of the Rust engine's own tools (not in Python's frozen catalog).
pub fn is_addition(name: &str) -> bool {
    additions().iter().any(|a| a.name == name)
}

/// Whether a listing of Python's tools (`listed`, names) came from a grant an addition is offered to, judged from
/// the listing alone, for the tests that compare recorded listings (the grant itself isn't recorded there): an
/// addition that acts only where the listing holds a catalog tool that acts in one of its scopes (an observing grant,
/// the checker's, holds none); one not offered to FERB's own identity only where the listing holds no tool that is
/// FERB's identity's alone (HAND-623: project.publish pushes with the owner's login); one that neither acts nor only
/// observes (MEM-448: memory.import) only where the listing holds a tool that acts in its scopes. Its `after` and
/// `named_only` are the caller's.
pub fn offered_beside(addition: &Addition, listed: &[String]) -> bool {
    let rows = catalog().raw()["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let row_of = |n: &str| rows.iter().find(|t| t["name"] == n).cloned();
    let scopes = strings(&addition.row["scopes"]);
    let identities = strings(&addition.row["identities"]);
    if addition.row["acts"].as_bool().unwrap_or(false) {
        let acting = listed.iter().filter_map(|n| row_of(n)).any(|t| {
            t["acts"].as_bool().unwrap_or(false) && !strings(&t["scopes"]).is_disjoint(&scopes)
        });
        if !acting {
            return false;
        }
    } else if !addition.row["facts"]["observation"]
        .as_bool()
        .unwrap_or(true)
    {
        // MEM-448: one that only does FERB's own work (memory.import) is never an observing grant's: offered only
        // where the listing holds a tool that acts in one of its scopes
        let acting = listed.iter().filter_map(|n| row_of(n)).any(|t| {
            t["acts"].as_bool().unwrap_or(false) && !strings(&t["scopes"]).is_disjoint(&scopes)
        });
        if !acting {
            return false;
        }
    }
    if !identities.contains("ferb") {
        let ferb_only = listed.iter().filter_map(|n| row_of(n)).any(|t| {
            let ids = strings(&t["identities"]);
            ids.len() == 1 && ids.contains("ferb")
        });
        if ferb_only {
            return false;
        }
    }
    true
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
