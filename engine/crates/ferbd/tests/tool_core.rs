//! ORCH-719 item 3, ORCH-752, ORCH-756: a worker's run is listed a fixed core of at most 25 tools in full and every
//! other tool by name and one line; the checker's at most 10. Through the composed `ferbd daemon` with the test
//! stand-ins (TEST-512): the listing the real program gives a run (the path every runtime takes:
//! `compose::rpc::tools_for`, which the MCP bridge's tools.list, the API loop's bridge and the worker's reserve all
//! call), the core chosen at the run's first listing and kept, the tools similar earlier tasks used, a direct call
//! of a tool listed by its line, and tools.find. Temp folders only; invented records (TEST-508).

mod shim;
mod support;

use std::collections::BTreeSet;

use ferb_gateway::tool_core::{self, CHECKER_CORE, FIND, WORKER_CORE};
use serde_json::{json, Value};
use shim::{all_scopes, Shim};
use support::*;

/// The tools a listing gives in full (a row whose schema is not the one-line rows' empty one).
fn in_full(listing: &Value) -> BTreeSet<String> {
    let lean = tool_core::listed_only_schema();
    listing
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["input_schema"] != lean)
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

fn run_listing(run: &Run, task: &str, worker: &str, observe_only: bool) -> Value {
    run.control(
        json!({"do": "listing", "run": true, "task_id": task, "worker_id": worker,
        "identity": "owner", "scopes": all_scopes(), "observe_only": observe_only}),
    )
}

/// The default core as the measured order gives it for an owner's run with every scope.
fn default_core(order: &[&str], size: usize, offered: &BTreeSet<String>) -> BTreeSet<String> {
    let mut core: BTreeSet<String> = order
        .iter()
        .filter(|n| offered.contains(**n))
        .take(size - 1)
        .map(|n| n.to_string())
        .collect();
    core.insert(FIND.into());
    core
}

/// `tools.list` over a run token's connection: what the MCP bridge hands Codex and Claude Code.
fn tools_list(run: &Run, token: &str) -> Value {
    let mut raw = Raw::connect(&run.socket()).unwrap();
    raw.send(
        &json!({"jsonrpc": "2.0", "id": 0, "method": "auth.hello",
                "params": {"role": "mcp_shim", "token": token}})
        .to_string(),
    );
    raw.recv().unwrap();
    raw.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools.list", "params": {}}).to_string());
    let v: Value = serde_json::from_str(&raw.recv().unwrap()).unwrap();
    v["result"]["tools"].clone()
}

/// An earlier request's task (`tree`) with these targets and these worker tool calls, as FERB records them.
fn earlier_task(run: &Run, tree: &str, targets: Value, tools: &[&str]) {
    let at = "2026-01-01T08:00:00+00:00";
    let mut statements = vec![
        (
            "INSERT OR IGNORE INTO conversations (id) VALUES ('owner')".to_string(),
            vec![],
        ),
        (
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
             created_at, updated_at) VALUES (?, ?, NULL, ?, 'g', 'owner', 'worker', '[]', 'done', ?, ?)"
                .to_string(),
            vec![json!(tree), json!(tree), json!(tree), json!(at), json!(at)],
        ),
        (
            "INSERT INTO task_context (task_id, conversation_id, spec_json, owner_words) VALUES (?, 'owner', ?, 'w')"
                .to_string(),
            vec![json!(tree), json!(json!({"targets": targets}).to_string())],
        ),
    ];
    for (i, t) in tools.iter().enumerate() {
        statements.push((
            "INSERT INTO spans (id, task_id, kind, started_at, input_json) VALUES (?, ?, 'tool.call', ?, ?)"
                .to_string(),
            vec![
                json!(format!("{tree}-s{i}")),
                json!(tree),
                json!(at),
                json!(json!({"tool": t, "args": {}}).to_string()),
            ],
        ));
    }
    run.control(json!({"do": "sql", "statements": statements}));
}

// decision: ORCH-719
// decision: ORCH-752
// decision: ORCH-756
#[test]
fn a_run_lists_its_core_in_full_and_every_other_tool_by_its_line_and_keeps_them() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id"}));
    run.control(json!({"do": "record_request", "request_id": "req-1", "words": "w"}));
    run.control(json!({"do": "hold_task", "task_id": "task-1", "request_id": "req-1"}));
    let first = run_listing(&run, "task-1", "w-1", false);
    let rows = first.as_array().unwrap();
    let offered: BTreeSet<String> = rows
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    let full = in_full(&first);
    // no similar task: the measured default order, tools.find among the 25
    assert_eq!(full.len(), WORKER_CORE);
    assert_eq!(
        full,
        default_core(tool_core::WORKER_ORDER, WORKER_CORE, &offered)
    );
    // every other tool: its name and its line, nothing else
    let catalog = ferb_gateway::catalog::catalog();
    for r in rows
        .iter()
        .filter(|r| !full.contains(r["name"].as_str().unwrap()))
    {
        let name = r["name"].as_str().unwrap();
        assert_eq!(r["description"], catalog.line(name), "{name}");
        assert!(!catalog.line(name).is_empty(), "{name} has a line");
        assert_eq!(r.as_object().unwrap().len(), 3, "{name}");
    }
    // the record of the run says what it got (the weekly guard reads it)
    let spans = run.table("spans");
    let recorded = spans
        .as_array()
        .unwrap()
        .iter()
        .find_map(|s| {
            // spans: id, parent_id, task_id, kind, device_id, started_at, ended_at, input_json, ...
            let input: Value = serde_json::from_str(s[7].as_str()?).ok()?;
            input.get("tool_core").cloned()
        })
        .expect("the run's span records its tools");
    assert_eq!(recorded["kind"], "worker");
    assert_eq!(recorded["full"].as_array().unwrap().len(), WORKER_CORE);
    assert_eq!(
        recorded["listed"].as_array().unwrap().len(),
        offered.len() - WORKER_CORE
    );

    // later steps: a similar task recorded meanwhile changes nothing for this run, byte for byte
    earlier_task(
        &run,
        "old-1",
        json!({"sites": ["example.com"]}),
        &["browser.pdf"],
    );
    run.control(
        json!({"do": "record_request", "request_id": "req-1", "words": "w",
        "targets": {"sites": ["example.com"]}}),
    );
    for _ in 0..3 {
        assert_eq!(run_listing(&run, "task-1", "w-1", false), first);
    }
    // and every runtime's path gives the same bytes: the MCP bridge's tools.list for this run's token
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
        "identity": "owner", "scopes": all_scopes()}),
    );
    assert_eq!(tools_list(&run, token.as_str().unwrap()), first);
}

// decision: ORCH-752
// decision: ORCH-756
#[test]
fn the_core_takes_what_similar_earlier_tasks_used_and_the_default_without_one() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id"}));
    // two earlier requests on the same site (written differently), one on another; worker calls outside the default
    earlier_task(
        &run,
        "old-1",
        json!({"sites": ["shop.example.com"], "apps": []}),
        &["browser.pdf", "browser.downloads", "browser.snapshot"],
    );
    earlier_task(
        &run,
        "old-2",
        json!({"sites": ["https://www.shop.example.com/cart"]}),
        &["browser.pdf"],
    );
    earlier_task(
        &run,
        "old-3",
        json!({"apps": ["Notes"]}),
        &["mac.applescript"],
    );
    run.control(
        json!({"do": "record_request", "request_id": "req-2", "words": "w",
        "targets": {"sites": ["shop.example.com"], "apps": [], "paths": [], "recipients": []}}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-2", "request_id": "req-2"}));
    let listing = run_listing(&run, "task-2", "w-2", false);
    let full = in_full(&listing);
    assert_eq!(full.len(), WORKER_CORE);
    assert!(full.contains("browser.pdf") && full.contains("browser.downloads"));
    assert!(
        !full.contains("mac.applescript"),
        "a task on another target is not similar"
    );
    let offered: BTreeSet<String> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    let default = default_core(tool_core::WORKER_ORDER, WORKER_CORE, &offered);
    let joined: Vec<&String> = full.difference(&default).collect();
    assert_eq!(joined, ["browser.downloads", "browser.pdf"]);
    // the two the default ranks last made room
    let left: Vec<&String> = default.difference(&full).collect();
    assert_eq!(left, ["fs.read", "mac.click"]);

    // a request on a target no earlier task had: the default core
    run.control(
        json!({"do": "record_request", "request_id": "req-3", "words": "w",
        "targets": {"sites": ["other.example.org"]}}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-3", "request_id": "req-3"}));
    assert_eq!(in_full(&run_listing(&run, "task-3", "w-3", false)), default);
}

// decision: ORCH-719
// decision: ORCH-756
#[test]
fn the_checker_gets_ten_in_full() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id"}));
    run.control(json!({"do": "record_request", "request_id": "req-1", "words": "w"}));
    run.control(json!({"do": "hold_task", "task_id": "task-1", "request_id": "req-1"}));
    let listing = run_listing(&run, "task-1", "check-1", true);
    let offered: BTreeSet<String> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    let full = in_full(&listing);
    assert_eq!(full.len(), CHECKER_CORE);
    assert_eq!(
        full,
        default_core(tool_core::CHECKER_ORDER, CHECKER_CORE, &offered)
    );
    assert!(offered.len() > CHECKER_CORE * 3, "{}", offered.len());
}

// decision: ORCH-719
// decision: ORCH-756
#[test]
fn a_tool_listed_by_its_line_runs_when_called_and_a_bad_call_gets_its_description() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id"}));
    std::fs::create_dir_all(run.home.join("workspace")).unwrap();
    run.control(json!({"do": "record_request", "request_id": "req-1", "words": "w"}));
    run.control(json!({"do": "hold_task", "task_id": "task-1", "request_id": "req-1"}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
        "identity": "owner", "scopes": all_scopes()}),
    );
    let listed = tools_list(&run, token.as_str().unwrap());
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "fs.disk_usage")
        .unwrap()
        .clone();
    assert_eq!(
        row["input_schema"],
        tool_core::listed_only_schema(),
        "listed by its line"
    );
    let mut shim = Shim::open(&run, token.as_str().unwrap());
    // a direct call with good arguments runs
    let ok = shim.call("fs.disk_usage", json!({"path": "."}));
    assert_eq!(ok["is_error"], false, "{ok}");
    // a bad one: the error carries the tool's full description, schema and all
    let bad = shim.call("fs.disk_usage", json!({"folder": "."}));
    assert_eq!(bad["is_error"], true);
    assert_eq!(bad["content"]["error"], "invalid arguments");
    let whole = &bad["content"]["tool"];
    assert_eq!(whole["name"], "fs.disk_usage");
    assert!(
        whole["input_schema"]["properties"]["path"].is_object(),
        "{whole}"
    );
    assert!(whole["description"]
        .as_str()
        .unwrap()
        .contains("metadata only"));
    // a tool of the core gets the plain error, as before
    let core_bad = shim.call("fs.read", json!({"file": "a.txt"}));
    assert_eq!(core_bad["content"]["error"], "invalid arguments");
    assert!(core_bad["content"].get("tool").is_none(), "{core_bad}");
}

// decision: ORCH-752
// decision: ORCH-756
#[test]
fn tools_find_gives_full_descriptions_by_name_or_by_words() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id"}));
    run.control(json!({"do": "record_request", "request_id": "req-1", "words": "w"}));
    run.control(json!({"do": "hold_task", "task_id": "task-1", "request_id": "req-1"}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
        "identity": "owner", "scopes": all_scopes()}),
    );
    let listed = tools_list(&run, token.as_str().unwrap());
    let mut shim = Shim::open(&run, token.as_str().unwrap());
    let by_name = shim.call(FIND, json!({"name": "browser.pdf"}));
    assert_eq!(by_name["is_error"], false, "{by_name}");
    let found = &by_name["content"]["tools"][0];
    let full = run.control(json!({"do": "listing", "identity": "owner", "scopes": all_scopes()}));
    let want = full
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "browser.pdf")
        .unwrap();
    assert_eq!(found, want, "the same row a full listing has");
    // the MCP programs' spelling names the same tool
    let mcp = shim.call(FIND, json!({"name": "browser_pdf"}));
    assert_eq!(&mcp["content"]["tools"][0], want);
    // words: the best matches in full
    let by_words = shim.call(FIND, json!({"query": "save the page as a PDF file"}));
    let names: Vec<&str> = by_words["content"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"browser.pdf"), "{names:?}");
    assert!(names.len() <= tool_core::FOUND_IN_FULL);
    // a tool the run isn't offered is not found
    let none = shim.call(FIND, json!({"name": "chat.dispatch_task"}));
    assert_eq!(none["is_error"], true, "{none}");
    // and tools.find itself is in the core
    assert!(in_full(&listed).contains(FIND));
}
