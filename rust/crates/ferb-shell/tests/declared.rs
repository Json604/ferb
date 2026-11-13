//! The eight shell tools' declared facts and `describe` against what the Python registry recorded in
//! `rust/fixtures/catalog/tools.json` (facts, scopes, identities) and tools.py's `describe`.

use ferb_shell::tools::{declared, describe, TOOLS};
use serde_json::{json, Map, Value};

fn catalog() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/catalog/tools.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// `facts` as the catalog writes them: only the attributes Python's class set beside the defaults it always writes.
fn facts_of(d: &ferb_shell::tools::Declared) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("risk".into(), json!(d.risk));
    m.insert("reversible".into(), json!(d.reversible));
    if d.observation {
        m.insert("observation".into(), json!(true));
    }
    if d.probe_safe {
        m.insert("probe_safe".into(), json!(true));
    }
    if let Some(o) = d.origin {
        m.insert("origin".into(), json!(o));
    }
    if !d.target_args.is_empty() {
        m.insert("target_args".into(), json!(d.target_args));
    }
    if !d.method_args.is_empty() {
        m.insert("method_args".into(), json!(d.method_args));
    }
    m.insert("on_screen".into(), json!(d.on_screen));
    if let Some(k) = d.ledger_key {
        m.insert("ledger_key".into(), json!(k));
    }
    m
}

fn sorted(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

#[test]
fn every_shell_tool_declares_what_the_catalog_recorded() {
    let cat = catalog();
    let tools = cat["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .filter(|n| n.starts_with("shell."))
        .collect();
    assert_eq!(names, TOOLS, "the registry's order");
    for name in TOOLS {
        let entry = tools.iter().find(|t| t["name"] == name).unwrap();
        let d = declared(name).unwrap();
        let want = entry["facts"].as_object().unwrap();
        let got = facts_of(&d);
        for (k, v) in want {
            assert_eq!(got.get(k), Some(v), "{name}: {k}");
        }
        assert_eq!(got.len(), want.len(), "{name}: {got:?} != {want:?}");
        let mut scopes: Vec<String> = d.scopes.iter().map(|s| s.to_string()).collect();
        scopes.sort();
        assert_eq!(scopes, sorted(&entry["scopes"]), "{name}");
        let mut ids: Vec<String> = d.identities.iter().map(|s| s.to_string()).collect();
        ids.sort();
        assert_eq!(ids, sorted(&entry["identities"]), "{name}");
        assert_eq!(
            !d.observation,
            entry["acts"].as_bool().unwrap(),
            "{name}: an acting tool is one that doesn't only observe"
        );
    }
    assert!(declared("fs.read").is_none() && describe("fs.read", &Map::new()).is_none());
}

#[test]
fn describe_names_the_kind_target_and_content_as_tools_py_does() {
    let args = |v: Value| v.as_object().unwrap().clone();
    let run = describe(
        "shell.run",
        &args(json!({"command": "ls -la", "cwd": null})),
    )
    .unwrap();
    assert_eq!(
        (
            run.kind.as_str(),
            run.target.path.as_deref(),
            run.content.as_deref()
        ),
        ("run_shell", Some("."), Some("ls -la"))
    );
    let start = describe(
        "shell.start",
        &args(json!({"command": "make", "cwd": "/tmp/x"})),
    )
    .unwrap();
    assert_eq!(start.target.path.as_deref(), Some("/tmp/x"));
    let empty = describe("shell.start", &args(json!({"command": "make", "cwd": ""}))).unwrap();
    assert_eq!(empty.target.path.as_deref(), Some("."), "`cwd or \".\"`");
    let send = describe("shell.send", &args(json!({"id": "p-1", "text": "y\n"}))).unwrap();
    assert_eq!(
        (send.kind.as_str(), send.content.as_deref()),
        ("run_shell", Some("y\n"))
    );
    assert_eq!(
        describe("shell.kill", &args(json!({"id": "p-1"})))
            .unwrap()
            .kind,
        "stop_process"
    );
    for t in ["shell.wait", "shell.read", "shell.list", "shell.rules"] {
        let s = describe(t, &Map::new()).unwrap();
        assert_eq!(
            (s.kind.as_str(), s.target.path.clone(), s.content.clone()),
            ("read", None, None)
        );
    }
}
