//! MEM-478 through the real program with the shipped pause (`import_paused` in the stand-ins' config; every other test
//! of this crate runs the import as if resumed): the workers aren't offered `memory.import`, and the import's RPCs say
//! it is paused while its status tells the clients to hide the controls.

mod shim;
mod support;

use serde_json::{json, Value};
use shim::{all_scopes, Shim};
use support::*;

fn rpc(run: &Run, method: &str, params: Value) -> Value {
    let mut c = Raw::connect(&run.socket()).expect("the socket");
    c.hello("cli").expect("hello");
    c.send(&json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}).to_string());
    serde_json::from_str(&c.recv().expect("an answer")).unwrap()
}

// decision: MEM-478
#[test]
fn the_paused_program_offers_no_memory_import_tool_and_its_rpcs_say_paused() {
    let tmp = root();
    let run = Run::start(
        tmp.path(),
        "01",
        json!({"ids": "id", "import_paused": true}),
    );
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
                                   "identity": "owner", "scopes": all_scopes()}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    let downloads = run.home.join("workspace/downloads/a1-claude-export");
    std::fs::create_dir_all(&downloads).unwrap();
    let mut shim = Shim::open(&run, token.as_str().unwrap());
    let reply = shim.call(
        "memory.import",
        json!({"source": "claude", "path": downloads.to_string_lossy(), "why": "the export the mail linked"}),
    );
    assert_eq!(reply["is_error"], true, "{reply}");
    let said = reply["content"].to_string();
    assert!(
        said.contains("no such tool") || said.contains("is not available to this worker"),
        "{reply}"
    );
    // a tool the build keeps still answers through the same connection
    let other = shim.call("abilities.list", json!({}));
    assert_eq!(other["is_error"], false, "{other}");
    let status = rpc(&run, "memory.import.status", json!({}));
    assert_eq!(status["result"]["paused"], json!(true), "{status}");
    let start = rpc(
        &run,
        "memory.import.start",
        json!({"source": "coding_assistants"}),
    );
    assert!(
        start["error"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("Bringing in what other assistants know is paused")),
        "{start}"
    );
    assert_eq!(run.table("memory_import_runs"), json!([]));
}
