//! The tool catalog against the frozen `rust/fixtures/catalog/tools.json` (ENG-3): which tools each of the 23 grants
//! is offered, in registry order, and the exact bytes of every listing as `json.dumps` writes it.
//!
//! The Rust engine's own tools (`catalog::additions`, each with its decision) are the one named difference: each
//! listing is Python's byte for byte with them taken out, and each addition is listed right after the tool it follows
//! wherever that tool is listed.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_gateway::catalog::{self, Live, SavedSecret};
use ferb_gateway::listing::tools_for;
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_rpc::auth::{Identity, RunGrant};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// A tool of the frozen catalog, by name only (nothing here runs it).
struct Frozen(String);

impl Tool for Frozen {
    fn name(&self) -> &str {
        &self.0
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec::new("read")
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async { Err(ToolError::failed("NotImplementedError", "")) })
    }
}

fn registry() -> Registry {
    let mut r = Registry::new();
    for e in &catalog::catalog().entries {
        r.add(Arc::new(Frozen(e.name.clone()))).unwrap();
    }
    r
}

fn set(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn every_listing_is_python_s_byte_for_byte() {
    let raw = catalog::catalog().raw();
    let r = registry();
    let listings = raw["listings"].as_object().unwrap();
    assert_eq!(listings.len(), 23);
    for (label, l) in listings {
        let g = &l["grant"];
        let grant = RunGrant {
            task_id: "t".into(),
            worker_id: "w".into(),
            scopes: set(&g["scopes"]),
            identity: if g["identity"] == "owner" {
                Identity::Owner
            } else {
                Identity::Ferb
            },
            expires_at: 1e9,
            observe_only: g["observe_only"].as_bool().unwrap(),
            tools: set(&g["tools"]),
            max_calls: g["max_calls"].as_u64().unwrap(),
        };
        let all = tools_for(&r, &grant, &Live::default());
        // decision: DMN-595: about.diagnostics follows about.read in every listing that holds it, unless the grant
        // names its tools without it
        let all_names: Vec<&str> = all.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for a in catalog::additions() {
            let named = grant.tools.is_empty() || grant.tools.contains(&a.name);
            let want = all_names.contains(&a.after.as_str()) && named;
            match all_names.iter().position(|n| *n == a.name) {
                Some(i) => {
                    assert!(want, "{label}: {} listed", a.name);
                    assert_eq!(all_names[i - 1], a.after, "{label}");
                }
                None => assert!(!want, "{label}: {} not listed", a.name),
            }
        }
        let listed: Vec<Value> = all
            .into_iter()
            .filter(|t| !catalog::is_addition(t["name"].as_str().unwrap()))
            .collect();
        let names: Vec<&str> = listed.iter().map(|t| t["name"].as_str().unwrap()).collect();
        let want: Vec<&str> = l["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect();
        assert_eq!(names, want, "{label}");
        let text = ferb_py::json::dumps(&Value::Array(listed));
        assert_eq!(
            text.len() as u64,
            l["json_default_len"].as_u64().unwrap(),
            "{label}"
        );
        assert_eq!(
            hex::encode(Sha256::digest(text.as_bytes())),
            l["json_default_sha256"].as_str().unwrap(),
            "{label}"
        );
    }
}

#[test]
fn every_tool_row_is_python_s() {
    let raw = catalog::catalog().raw();
    let r = registry();
    for t in raw["tools"].as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        let tool = r.get(name).unwrap();
        let schema = ferb_gateway::listing::listed_schema(tool.as_ref());
        assert_eq!(schema, t["input_schema"], "{name}");
        // key order is contract: compare the bytes json.dumps writes
        assert_eq!(
            ferb_py::json::dumps(&schema),
            ferb_py::json::dumps(&t["input_schema"]),
            "{name}"
        );
        let forms = &t["input_schema_forms"];
        let text = ferb_py::json::dumps(&schema);
        assert_eq!(
            hex::encode(Sha256::digest(text.as_bytes())),
            forms["default_sha256"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            tool.description(&Live::default()),
            t["description"].as_str().unwrap()
        );
        assert_eq!(
            ferb_gateway::listing::acts(tool.as_ref()),
            t["acts"].as_bool().unwrap(),
            "{name}"
        );
    }
}

// decision: DMN-595
#[test]
fn the_rust_additions_are_listed_as_written() {
    let r = registry();
    assert_eq!(
        catalog::catalog().entries.len(),
        catalog::catalog().raw()["tools"].as_array().unwrap().len() + catalog::additions().len()
    );
    let a = catalog::additions();
    assert_eq!(
        a.iter()
            .map(|a| (a.decision.as_str(), a.name.as_str()))
            .collect::<Vec<_>>(),
        [("DMN-595", "about.diagnostics")]
    );
    for a in a {
        let tool = r.get(&a.name).unwrap();
        let schema = ferb_gateway::listing::listed_schema(tool.as_ref());
        assert_eq!(
            ferb_py::json::dumps(&schema),
            ferb_py::json::dumps(&a.row["input_schema"]),
            "{}",
            a.name
        );
        assert_eq!(
            tool.description(&Live::default()),
            a.row["description"].as_str().unwrap()
        );
        assert_eq!(
            ferb_gateway::listing::acts(tool.as_ref()),
            a.row["acts"].as_bool().unwrap()
        );
        let f = tool.facts();
        assert!(
            f.observation && f.reversible && !f.generic_input,
            "{}",
            a.name
        );
        assert_eq!(f.origin.as_deref(), Some("ferb"));
        assert!(tool.scopes().is_empty());
    }
}

#[test]
fn descriptions_that_read_live_facts_are_composed_as_python_does() {
    let raw = catalog::catalog().raw();
    let live = Live {
        secrets: vec![SavedSecret {
            key: "ferb/example/password".into(),
            account_hint: "ferb@example.test".into(),
            allowed_origins: vec![
                "https://accounts.example.test".into(),
                "https://example.test".into(),
            ],
        }],
        downloads_total_bytes: None,
    };
    let want = raw["dynamic_descriptions"]["with_one_invented_secret"]
        .as_object()
        .unwrap();
    for (name, text) in want {
        assert_eq!(
            catalog::description(name, &live).unwrap(),
            text.as_str().unwrap()
        );
    }
    let five = catalog::description(
        "browser.downloads",
        &Live {
            downloads_total_bytes: Some(5_500_000_000),
            ..Live::default()
        },
    )
    .unwrap();
    assert!(five.contains("passes 5.5 GB"), "{five}");
}
