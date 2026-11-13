//! The composed registry against the frozen catalog (rust/fixtures/catalog/tools.json, made by Python's
//! `Gateway.tools_for`): all 97 tools in registry order, and the 23 listings by grant byte for byte (their
//! `json.dumps` SHA-256). Then `ferbd mcp` over stdio with a run token: the tool bridge lists the catalog for it.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::*;

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[test]
fn the_registry_is_the_frozen_catalog() {
    let catalog = fixture("catalog/tools.json");
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({}));
    // FERB's own profile attached, as the recorder's daemon had it (its 21 tools come last)
    run.control(json!({"do": "attach_identity"}));
    let full = run.control(json!({"do": "listing", "identity": "owner",
        "scopes": catalog["listings"]["worker_owner_all_scopes"]["grant"]["scopes"]}));
    let ferb = run.control(json!({"do": "listing", "identity": "ferb",
        "scopes": catalog["listings"]["worker_ferb_all_scopes"]["grant"]["scopes"]}));
    // every catalog tool, once, in the registry's order
    let mut names: Vec<String> = Vec::new();
    for l in [&full, &ferb] {
        for t in l.as_array().unwrap() {
            let n = t["name"].as_str().unwrap().to_string();
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    let want: Vec<String> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    let mut seen = names.clone();
    seen.sort();
    let mut all = want.clone();
    all.sort();
    assert_eq!(seen, all, "the 97 tools");
    assert_eq!(want.len(), 97);
    // every one with a real adapter: none answers "not available in this build"
    assert_eq!(
        run.control(json!({"do": "refusing"})),
        json!([]),
        "tools still refusing"
    );
    // each listing by grant: names in order, and the whole listing's bytes
    let mut checked = 0;
    for (name, l) in catalog["listings"].as_object().unwrap() {
        let g = &l["grant"];
        let got = run.control(
            json!({"do": "listing", "identity": g["identity"], "scopes": g["scopes"],
            "observe_only": g["observe_only"], "tools": g["tools"], "max_calls": g["max_calls"]}),
        );
        let got_names: Vec<&str> = got
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        let want_names: Vec<&str> = l["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect();
        assert_eq!(got_names, want_names, "{name}: names");
        let text = ferb_py::json::dumps(&got);
        assert_eq!(
            sha(&text),
            l["json_default_sha256"],
            "{name}: listing bytes"
        );
        checked += 1;
    }
    assert_eq!(checked, 23);
}

#[test]
fn ferbd_mcp_lists_the_catalog_for_a_run_token() {
    let catalog = fixture("catalog/tools.json");
    let tmp = root();
    let run = Run::start(tmp.path(), "02", json!({}));
    let g = &catalog["listings"]["planner_lookups"]["grant"];
    let token = run.control(json!({"do": "issue", "task_id": "task-1", "worker_id": "planner-1",
        "identity": g["identity"], "scopes": g["scopes"], "observe_only": g["observe_only"], "tools": g["tools"],
        "max_calls": g["max_calls"]}));
    let mut child = Command::new(BIN)
        .arg("mcp")
        .env_clear()
        .env("FERB_HOME", &run.home)
        .env("HOME", &run.user)
        .env("FERB_RUN_TOKEN", token.as_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    fn ask(stdin: &mut impl Write, out: &mut impl BufRead, line: Value) -> Value {
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        let mut answer = String::new();
        out.read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap_or_else(|_| panic!("not JSON: {answer:?}"))
    }
    let init = ask(
        &mut stdin,
        &mut out,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .unwrap();
    let listed = ask(
        &mut stdin,
        &mut out,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    // each tool exactly {name: mcp_name, description, inputSchema} (catalog README); the envelope is what the
    // bridge's own fixture (rust/fixtures/runtimes/mcp.json) has the mcp library write on the wire
    let tools = |v: &Value| -> Vec<Value> {
        v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| json!([t["name"], t["description"], t["inputSchema"]]))
            .collect()
    };
    let want = tools(&catalog["mcp_tools_list"]["planner_example"]);
    assert_eq!(want.len(), 6);
    assert_eq!(tools(&listed["result"]), want, "{listed}");
    drop(stdin);
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));

    // and with no token the bridge refuses, as Python's does
    let none = Command::new(BIN)
        .arg("mcp")
        .env_clear()
        .env("FERB_HOME", &run.home)
        .output()
        .unwrap();
    assert_eq!(none.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&none.stderr).contains("FERB_RUN_TOKEN is not set"));
}
