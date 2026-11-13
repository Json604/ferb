//! Shared helpers for the parity tests: the fixtures `rust/fixtures/policy/make.py` wrote, and Python's shapes.

#![allow(dead_code)]

use std::cell::RefCell;
use std::fs;
use std::path::Path;

use ferb_core::types::Target;
use ferb_policy::confirm::{Apps, AppsUnread, Need, RunningApp, Values};
use serde_json::{json, Map, Value};

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/policy")
        .join(name);
    serde_json::from_str(&fs::read_to_string(path).expect("the fixture exists"))
        .expect("the fixture is JSON")
}

/// JSON values equal as Python compares them: numbers by value (1 == 1.0), objects in any key order.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q)))
        }
        _ => a == b,
    }
}

/// Count a difference: print the first few.
pub struct Diffs {
    pub cases: usize,
    pub failed: Vec<String>,
}

impl Diffs {
    pub fn new() -> Diffs {
        Diffs {
            cases: 0,
            failed: Vec::new(),
        }
    }

    pub fn check(&mut self, what: &str, input: &Value, want: &Value, got: &Value) {
        self.cases += 1;
        if !same(want, got) {
            self.failed.push(format!(
                "{what}\n  input: {}\n  python: {}\n  rust:   {}",
                cut(&input.to_string()),
                cut(&want.to_string()),
                cut(&got.to_string())
            ));
        }
    }

    pub fn finish(self, section: &str) {
        eprintln!(
            "{section}: {} cases, {} differ",
            self.cases,
            self.failed.len()
        );
        if !self.failed.is_empty() {
            for f in self.failed.iter().take(12) {
                eprintln!("{f}");
            }
            panic!(
                "{section}: {} of {} cases differ from Python",
                self.failed.len(),
                self.cases
            );
        }
    }
}

fn cut(s: &str) -> String {
    if s.chars().count() > 700 {
        format!("{}…", s.chars().take(700).collect::<String>())
    } else {
        s.to_string()
    }
}

pub fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

pub fn opt_s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

pub fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

pub fn ok(v: Value) -> Value {
    json!({ "ok": v })
}

/// Python's `outcome`: {"ok": …} or {"raises": …}; a Rust Err is compared by its being one.
pub fn raised(want: &Value) -> bool {
    want.get("raises").is_some()
}

pub fn target(v: &Value) -> Option<Target> {
    if v.is_null() {
        return None;
    }
    Some(Target {
        account: opt_s(&v["account"]),
        site: opt_s(&v["site"]),
        recipients: strs(&v["recipients"]),
        path: opt_s(&v["path"]),
        amount: v["amount"].as_f64(),
        currency: opt_s(&v["currency"]),
        contact: opt_s(&v["contact"]),
        page: opt_s(&v["page"]),
        item: opt_s(&v["item"]),
    })
}

pub fn target_json(t: Option<&Target>) -> Value {
    match t {
        None => Value::Null,
        Some(t) => {
            json!({"account": t.account, "site": t.site, "recipients": t.recipients, "path": t.path,
            "amount": t.amount, "currency": t.currency, "contact": t.contact, "page": t.page, "item": t.item})
        }
    }
}

pub fn values(v: &Value) -> Values {
    let mut out = Values::new();
    match v {
        Value::Array(pairs) => {
            for p in pairs {
                out.set(&s(&p[0]), &s(&p[1]));
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                out.set(k, &s(x));
            }
        }
        _ => {}
    }
    out
}

pub fn need(v: &Value) -> Need {
    let mut n = Need::new(&s(&v["rule"]), s(&v["key"]), values(&v["values"]));
    n.always = v["always"].as_bool().unwrap_or(false);
    n.destinations = v["destinations"]
        .as_array()
        .map(|a| a.iter().map(|d| (s(&d[0]), s(&d[1]))).collect())
        .unwrap_or_default();
    n.fresh = v["fresh"].as_bool().unwrap_or(false);
    n.spent_on_ask = v["spent_on_ask"].as_bool().unwrap_or(false);
    n.reaches = target(&v["reaches"]);
    n.grants = s(&v["grants"]);
    n
}

pub fn need_json(n: &Need) -> Value {
    json!({"rule": n.rule, "key": n.key,
        "values": n.values.0.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "always": n.always, "destinations": n.destinations.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
        "fresh": n.fresh, "spent_on_ask": n.spent_on_ask, "reaches": target_json(n.reaches.as_ref()), "grants": n.grants})
}

/// The fake Mac apps, answering exactly what Python's fake answered (its call log); a question Python never asked
/// fails the test.
pub struct LogApps {
    log: Vec<Value>,
    pub missed: RefCell<Vec<String>>,
}

impl LogApps {
    pub fn new(log: &Value) -> LogApps {
        LogApps {
            log: log.as_array().cloned().unwrap_or_default(),
            missed: RefCell::new(Vec::new()),
        }
    }

    fn find(&self, kind: &str, q: &str) -> Option<&Value> {
        let hit = self
            .log
            .iter()
            .find(|e| e[0] == kind && e[1] == q)
            .map(|e| &e[2]);
        if hit.is_none() {
            self.missed.borrow_mut().push(format!("{kind} {q:?}"));
        }
        hit
    }
}

impl Apps for LogApps {
    fn candidates(&self, name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        match self.find("candidates", name) {
            Some(Value::Array(found)) => Ok(found
                .iter()
                .map(|f| RunningApp {
                    bundle_id: s(&f[0]),
                    name: s(&f[1]),
                })
                .collect()),
            _ => Err(AppsUnread),
        }
    }

    fn installed_bundle(&self, name: &str) -> Result<Option<String>, AppsUnread> {
        match self.find("installed", name) {
            Some(Value::String(b)) if b == "error" => Err(AppsUnread),
            Some(Value::String(b)) => Ok(Some(b.clone())),
            Some(Value::Null) => Ok(None),
            _ => Err(AppsUnread),
        }
    }

    fn is_browser_app(&self, app: &str) -> bool {
        matches!(self.find("browser", app), Some(Value::Bool(true)))
    }
}

pub fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v.clone());
    }
    Value::Object(m)
}
