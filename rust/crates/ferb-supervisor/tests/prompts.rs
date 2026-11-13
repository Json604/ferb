//! `_system` (supervisor.py:1230) against the frozen catalog (rust/fixtures/catalog/prompt_static.json: 16 calls ×
//! two input sets) and against every system text the Python daemon sent in the 28 chat scenarios (the 118 model
//! calls of rust/fixtures/daemon/chat_scenarios.json, with the inputs make.py recorded at each call).

mod common;

use common::*;
use ferb_supervisor::prompts::{catalog_line, runtime_tools_block, system, SystemInputs};
use serde_json::Value;

fn roles(v: &Value) -> Vec<String> {
    strs(v)
}

#[test]
fn prompt_static_systems_byte_for_byte() {
    let fx = read("rust/fixtures/catalog/prompt_static.json");
    let root = root();
    let mut checked = 0;
    for variant in ["minimal", "rich"] {
        let v = &fx["variants"][variant];
        let i = &v["inputs"];
        let lines = fx["catalog_lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["what"] == variant)
            .expect("the variant's catalog lines");
        let inputs = SystemInputs {
            facts: s(&i["facts"]),
            live_facts: s(&i["live_facts"]),
            capabilities: s(&i["capabilities"]),
            identity_context: s(&i["identity_context"]),
            skills_line: s(&lines["skills_line"]),
            abilities_line: s(&lines["abilities_line"]),
        };
        for sys in v["systems"].as_array().unwrap() {
            let name = sys["name"].as_str();
            let r = roles(&sys["roles"]);
            let r: Vec<&str> = r.iter().map(String::as_str).collect();
            let got = system(&root, name, sys["live"].as_bool().unwrap(), &r, &inputs).unwrap();
            assert_eq!(got, s(&sys["system"]), "{variant}: {}", s(&sys["what"]));
            checked += 1;
        }
    }
    assert_eq!(checked, 32);
}

/// The catalogs as `json.dumps` writes them. The rich variant's abilities hold an integer of 30 digits, which
/// `serde_json::Value` can't hold (it reads as a float): the one input shape not reproduced (no catalog FERB builds
/// holds a number beyond 64 bits).
#[test]
fn catalog_lines_as_python_writes_them() {
    let fx = read("rust/fixtures/catalog/prompt_static.json");
    for variant in ["minimal", "rich"] {
        let i = &fx["variants"][variant]["inputs"];
        let lines = fx["catalog_lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["what"] == variant)
            .unwrap();
        assert_eq!(
            catalog_line(i["skills_catalog"].as_array().unwrap()),
            s(&lines["skills_line"]),
            "{variant}"
        );
        let abilities = catalog_line(i["abilities_catalog"].as_array().unwrap());
        if variant == "minimal" {
            assert_eq!(abilities, s(&lines["abilities_line"]));
        } else {
            let want = s(&lines["abilities_line"])
                .replace("123456789012345678901234567890", "1.2345678901234568e+29");
            assert_eq!(
                abilities, want,
                "the rich catalog apart from its 30-digit integer"
            );
        }
    }
}

#[test]
fn runtime_tools_blocks() {
    let fx = read("rust/fixtures/catalog/prompt_static.json");
    let root = root();
    for case in fx["runtime_tools_blocks"].as_array().unwrap() {
        let names = s(&case["tool_names"]);
        let names: Vec<String> = if names == "none" {
            Vec::new()
        } else {
            names.split(", ").map(str::to_string).collect()
        };
        let got = runtime_tools_block(&root, &names, &s(&case["time_block"])).unwrap();
        assert_eq!(got, s(&case["text"]), "{}", s(&case["what"]));
    }
}

#[test]
fn every_recorded_system_text() {
    let root = root();
    let mut checked = 0;
    let daemon = read("rust/fixtures/daemon/chat_scenarios.json");
    for (name, records) in scenarios() {
        let systems: Vec<&Value> = records.iter().filter(|r| r["kind"] == "system").collect();
        for r in &systems {
            let i = &r["inputs"];
            let inputs = SystemInputs {
                facts: s(&i["facts"]),
                live_facts: s(&i["live_facts"]),
                capabilities: s(&i["capabilities"]),
                identity_context: s(&i["identity_context"]),
                skills_line: catalog_line(&catalog(&i["skills_catalog"])),
                abilities_line: catalog_line(&catalog(&i["abilities_catalog"])),
            };
            let rl = roles(&r["roles"]);
            let rl: Vec<&str> = rl.iter().map(String::as_str).collect();
            let got = system(
                &root,
                r["name"].as_str(),
                r["live"].as_bool().unwrap(),
                &rl,
                &inputs,
            )
            .unwrap();
            assert_eq!(got, s(&r["system"]), "{name}: {:?} {:?}", r["name"], rl);
            checked += 1;
        }
        // and each is what the model received, call by call
        let calls = daemon["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sc| sc["name"] == name.as_str())
            .unwrap();
        let sent: Vec<Value> = calls["model_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| resolve(&c["system"]))
            .collect();
        let made: Vec<Value> = systems.iter().map(|r| r["system"].clone()).collect();
        assert_eq!(
            made, sent,
            "{name}: the systems in the order the model received them"
        );
    }
    assert_eq!(checked, 118);
}
