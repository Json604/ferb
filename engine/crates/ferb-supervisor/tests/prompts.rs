//! `_system` (supervisor.py:1230) against the frozen catalog (engine/fixtures/catalog/prompt_static.json: 16 calls ×
//! two input sets) and against every system text the Python daemon sent in the 28 chat scenarios (the 118 model
//! calls of engine/fixtures/daemon/chat_scenarios.json, with the inputs make.py recorded at each call).

mod common;

use common::*;
use ferb_supervisor::prompts::{catalog_line, runtime_tools_block, system, SystemInputs};
use serde_json::Value;

fn roles(v: &Value) -> Vec<String> {
    strs(v)
}

#[test]
fn prompt_static_systems_byte_for_byte() {
    // the recorded systems with the deliberate prompt changes since (ENG-6, engine/fixtures/catalog/prompt_changes.json)
    let fx = prompt_changes::apply_value(&read("engine/fixtures/catalog/prompt_static.json"));
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
    let fx = read("engine/fixtures/catalog/prompt_static.json");
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
    let fx = read("engine/fixtures/catalog/prompt_static.json");
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
    let daemon = read("engine/fixtures/daemon/chat_scenarios.json");
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

/// Each deliberate prompt change (engine/fixtures/catalog/prompt_changes.json) is in its prompt file now, its old text
/// is gone from it, and it changes recorded texts: the recordings' other bytes are compared unchanged. A change may
/// change an earlier change's text again.
// decision: ORCH-640, ORCH-663
#[test]
fn the_deliberate_prompt_changes_are_the_only_ones() {
    let dir = repo().join("engine/runtime/orchestrator/prompts");
    let fx = read("engine/fixtures/catalog/prompt_static.json");
    let recorded: Vec<String> = fx["variants"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|v| {
            v["systems"]
                .as_array()
                .unwrap()
                .iter()
                .map(|sys| s(&sys["system"]))
        })
        .collect();
    let file = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(repo().join("engine/fixtures/catalog/prompt_changes.json"))
            .unwrap(),
    )
    .unwrap();
    let all = prompt_changes::changes();
    let prompt_of = |n: usize| s(&raw["changes"][n]["prompt"]);
    for (n, (decision, old, new)) in all.iter().enumerate() {
        let prompt = prompt_of(n);
        let now = file(&prompt);
        // a later change of the same prompt may change this one's text again (ORCH-663 on ORCH-640's line): its new
        // text is in the file with those later changes applied, in order
        let mut became = new.clone();
        for (m, (_, o, w)) in all.iter().enumerate().skip(n + 1) {
            if prompt_of(m) == prompt && became.contains(o.as_str()) {
                became = became.replace(o.as_str(), w);
            }
        }
        assert!(
            now.contains(became.as_str()),
            "{decision}: the new text is in {prompt}"
        );
        assert!(
            !now.contains(old.as_str()),
            "{decision}: the old text is gone from {prompt}"
        );
        // the old text is what the recording holds, or what an earlier change of the same prompt made
        assert!(
            recorded.iter().any(|r| r.contains(old.as_str()))
                || all[..n]
                    .iter()
                    .enumerate()
                    .any(|(m, (_, _, w))| prompt_of(m) == prompt && w.contains(old.as_str())),
            "{decision}: the old text is what the recording holds"
        );
    }
}
