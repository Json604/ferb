//! The tool catalog against the frozen `engine/fixtures/catalog/tools.json` (ENG-3): which tools each of the 23 grants
//! is offered, in registry order, and the exact bytes of every listing as `json.dumps` writes it.
//!
//! The Rust engine's own tools (`catalog::additions`, each with its decision) are the one named difference: each
//! listing is Python's byte for byte with them taken out, and each addition is listed right after the tool it follows
//! wherever that tool is listed. The deliberate changes to Python's tools (`catalog::changes`, each with its decision;
//! HAND-551 mac.act's element actions) are the other: taken out of a listing before its bytes are compared, applied to
//! a recorded row.

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
        // ORCH-663: one offered only by name (task.trace) is listed only where the grant names it
        for a in catalog::additions() {
            let only_named = r.get(&a.name).unwrap().facts().named_only;
            let named = (grant.tools.is_empty() && !only_named) || grant.tools.contains(&a.name);
            // HAND-623: project.publish pushes with the owner's login, so only an owner grant is offered it
            let who = if grant.identity == Identity::Owner {
                "owner"
            } else {
                "ferb"
            };
            let as_whom = r.get(&a.name).unwrap().identities().contains(who);
            // an observing grant (the checker's) is offered no tool that acts, nor one that only FERB's own work
            // does (MEM-448: memory.import fills FERB's memory)
            let tool = r.get(&a.name).unwrap();
            let observing = grant.observe_only
                && (ferb_gateway::listing::acts(tool.as_ref()) || !tool.facts().observation);
            // MEM-448: memory.import follows browser.downloads, where the grant reads the browser its downloads come from
            let scoped = tool.scopes().is_subset(&grant.scopes);
            let want =
                all_names.contains(&a.after.as_str()) && named && as_whom && !observing && scoped;
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
        // decision: HAND-551: the listing with the deliberate changes taken out is Python's
        let text = catalog::revert_changes(&ferb_py::json::dumps(&Value::Array(listed)));
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
        // key order is contract: compare the bytes json.dumps writes; the deliberate changes (HAND-551) applied to
        // the recorded row
        assert_eq!(
            ferb_py::json::dumps(&schema),
            catalog::apply_changes(&ferb_py::json::dumps(&t["input_schema"])),
            "{name}"
        );
        let forms = &t["input_schema_forms"];
        let text = catalog::revert_changes(&ferb_py::json::dumps(&schema));
        assert_eq!(
            hex::encode(Sha256::digest(text.as_bytes())),
            forms["default_sha256"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            tool.description(&Live::default()),
            catalog::apply_changes(t["description"].as_str().unwrap())
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
        [
            ("DMN-595", "about.diagnostics"),
            ("ORCH-663", "task.trace"),
            ("HAND-623", "project.publish"),
            ("HAND-642", "project.fetch"),
            ("SENSE-422", "device.facts"),
            ("MEM-448", "memory.import"),
            ("ORCH-736", "chat.ask"),
            ("ORCH-736", "chat.control_task"),
            ("ORCH-736", "chat.dispatch_task"),
            ("ORCH-736", "chat.remember"),
            ("ORCH-736", "chat.schedule"),
            ("ORCH-736", "chat.set_reminder")
        ]
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
        // the read-only additions observe; project.publish (HAND-623) acts and can't be undone; memory.import
        // (MEM-448) neither acts on the world nor only observes: it fills FERB's own memory, reversibly, as
        // time.remind_at sets FERB's own timer; so do the front door's six (ORCH-736): each hands a part of the
        // chat turn's answer to the supervisor, which applies it as today's answer form
        let acting = a.row["acts"].as_bool().unwrap();
        let internal = a.name == "memory.import" || a.name.starts_with("chat.");
        assert!(
            (f.observation != acting || internal) && f.reversible != acting && !f.generic_input,
            "{}",
            a.name
        );
        assert_eq!(
            f.origin.as_deref(),
            a.row["facts"]["origin"].as_str(),
            "{}",
            a.name
        );
        assert_eq!(
            tool.scopes(),
            a.row["scopes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect(),
            "{}",
            a.name
        );
    }
}

/// ORCH-663 (the owner, 2026-11-16): task.trace is the planner's: offered and admitted only to a grant that names it
/// (the planner's lookups); a worker's grant (every scope, no tool named) and the check's (observe-only) get neither.
// decision: ORCH-663
#[test]
fn the_trace_tool_is_offered_and_admitted_only_to_a_grant_naming_it() {
    let r = registry();
    let tool = r.get("task.trace").unwrap();
    assert!(tool.facts().named_only && tool.facts().observation);
    let grant = |tools: &[&str], observe_only: bool| RunGrant {
        task_id: "t".into(),
        worker_id: "w".into(),
        scopes: ["task:read", "trail:read", "fs:read", "browser:read"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only,
        tools: tools.iter().map(|s| s.to_string()).collect(),
        max_calls: 0,
    };
    let planner = grant(&ferb_orchestrator::lookups::PLANNER_TOOLS, true);
    let worker = grant(&[], false);
    let checker = grant(&[], true);
    let names = |g: &RunGrant| -> Vec<String> {
        tools_for(&r, g, &Live::default())
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(names(&planner).contains(&"task.trace".to_string()));
    for g in [&worker, &checker] {
        let listed = names(g);
        assert!(!listed.contains(&"task.trace".to_string()), "{listed:?}");
        assert!(
            listed.contains(&"task.evidence".to_string()),
            "its equivalent stays"
        );
        assert!(
            ferb_gateway::admit::admit(g, Some(tool.as_ref()), "task.trace", false, true, 0)
                .is_err()
        );
    }
    assert!(ferb_gateway::admit::admit(
        &planner,
        Some(tool.as_ref()),
        "task.trace",
        false,
        true,
        0
    )
    .is_ok());
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

// decision: HAND-551
// decision: GATE-621
// decision: MEM-441
// decision: HAND-676
// decision: HAND-681
// decision: HAND-724
#[test]
fn the_deliberate_changes_are_the_only_ones() {
    let raw = catalog::catalog().raw();
    let c = catalog::changes();
    assert_eq!(
        c.iter()
            .map(|c| (c.decision.as_str(), c.tool.as_str(), c.part.as_str()))
            .collect::<Vec<_>>(),
        [
            ("HAND-551", "mac.act", "input_schema"),
            ("HAND-551", "mac.act", "description"),
            ("HAND-551", "mac.act", "validation"),
            ("GATE-621", "browser.act", "input_schema"),
            ("GATE-621", "ferb_browser.act", "input_schema"),
            ("GATE-621", "mac.act", "input_schema"),
            ("MEM-441", "memory.learned", "description"),
            ("HAND-676", "mac.snapshot", "description"),
            ("HAND-681", "mac.snapshot", "description"),
            ("HAND-681", "mac.snapshot", "input_schema"),
            ("HAND-724", "browser.close", "input_schema"),
            ("HAND-724", "ferb_browser.close", "input_schema"),
            ("HAND-724", "browser.close", "description"),
            ("HAND-724", "ferb_browser.close", "description"),
        ]
    );
    for c in c.iter().filter(|c| c.part != "validation") {
        let t = raw["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == c.tool.as_str())
            .unwrap();
        let recorded = match c.part.as_str() {
            "description" => t["description"].as_str().unwrap().to_string(),
            _ => ferb_py::json::dumps(&t["input_schema"]),
        };
        // the old text is Python's, once; the new one is what the catalog serves now
        assert_eq!(recorded.matches(c.old.as_str()).count(), 1, "{}", c.part);
        let e = catalog::catalog().entry(&c.tool).unwrap();
        let now = match c.part.as_str() {
            "description" => e.description.clone(),
            _ => ferb_py::json::dumps(&e.schema),
        };
        assert!(now.contains(c.new.as_str()), "{}", c.part);
        // taken out again, Python's text (the recorded schema also holds what the gateway adds: compared as listed in
        // every_tool_row_is_python_s)
        if c.part == "description" {
            assert_eq!(catalog::revert_changes(&now), recorded);
        }
    }
    // HAND-724: browser.close takes the worker's owner_asked, false unless said
    for tool in ["browser.close", "ferb_browser.close"] {
        let e = catalog::catalog().entry(tool).unwrap();
        assert_eq!(
            e.schema["properties"]["owner_asked"],
            serde_json::json!({"default": false, "description": "true only when the owner's own words ask FERB to close \
                this tab although FERB didn't open it (it's his); FERB never closes his tabs otherwise.",
                "title": "Owner Asked", "type": "boolean"}),
            "{tool}"
        );
        assert_eq!(e.schema["required"], serde_json::json!(["tab"]), "{tool}");
    }
    // mac.act names its element actions: menu and perform with name
    let e = catalog::catalog().entry("mac.act").unwrap();
    assert_eq!(
        e.schema["properties"]["action"]["enum"],
        serde_json::json!([
            "press",
            "set_value",
            "focus",
            "click",
            "type",
            "key",
            "edit",
            "scroll",
            "select",
            "cancel",
            "menu",
            "perform"
        ])
    );
    assert_eq!(e.schema["properties"]["name"]["title"], "Name");
    assert!(e.description.contains("not by a right-click at a point"));
}

/// The owner (2026-11-16): the planner may look up this Mac's facts too; its grant (observe-only) is offered and admits
/// device.facts, as a worker's is.
// decision: SENSE-422
#[test]
fn the_device_facts_are_a_planner_lookup_and_a_workers_tool() {
    let r = registry();
    let tool = r.get("device.facts").unwrap();
    let grant = |tools: &[&str], observe_only: bool| RunGrant {
        task_id: "t".into(),
        worker_id: "w".into(),
        scopes: ferb_orchestrator::lookups::PLANNER_SCOPES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only,
        tools: tools.iter().map(|s| s.to_string()).collect(),
        max_calls: 0,
    };
    for g in [
        grant(&ferb_orchestrator::lookups::PLANNER_TOOLS, true),
        grant(&[], false),
    ] {
        let listed: Vec<String> = tools_for(&r, &g, &Live::default())
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(listed.contains(&"device.facts".to_string()), "{listed:?}");
        assert!(ferb_gateway::admit::admit(
            &g,
            Some(tool.as_ref()),
            "device.facts",
            false,
            true,
            0
        )
        .is_ok());
    }
}

/// ORCH-696: a check's grant (observe-only, no tool list) with its cap admits CHECK_CALLS calls and refuses the next
/// with the check's own words (give the verdict from what it has; a condition it couldn't confirm is null); the
/// planner's lookups keep theirs. Every call counts, opened records as much as looks.
// decision: ORCH-696
#[test]
fn a_checks_calls_past_its_cap_are_refused_with_the_checks_words() {
    let r = registry();
    let grant = |tools: &[&str]| RunGrant {
        task_id: "t".into(),
        worker_id: "w".into(),
        scopes: ["task:read", "browser:read"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: true,
        tools: tools.iter().map(|s| s.to_string()).collect(),
        max_calls: 6,
    };
    let check = grant(&[]);
    for name in ["task.evidence", "browser.snapshot"] {
        let tool = r.get(name).unwrap();
        for made in 0..6 {
            assert!(ferb_gateway::admit::admit(
                &check,
                Some(tool.as_ref()),
                name,
                false,
                true,
                made
            )
            .is_ok());
        }
        let refused = ferb_gateway::admit::admit(&check, Some(tool.as_ref()), name, false, true, 6)
            .unwrap_err();
        assert!(refused.over_cap);
        assert_eq!(
            refused.content["error"],
            "this check has made its 6 calls: give your verdict now from what you have; a condition you couldn't \
             confirm is met: null"
        );
    }
    let planner = grant(&ferb_orchestrator::lookups::PLANNER_TOOLS);
    let tool = r.get("task.evidence").unwrap();
    let refused = ferb_gateway::admit::admit(
        &planner,
        Some(tool.as_ref()),
        "task.evidence",
        false,
        true,
        6,
    )
    .unwrap_err();
    assert!(refused.content["error"]
        .as_str()
        .unwrap()
        .starts_with("this turn has made its 6 lookups"));
}
