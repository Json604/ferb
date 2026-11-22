//! MEM-448, MEM-440, MEM-451: the workers' `memory.import` through the real binary: a run token's call over the
//! shim's socket path reaches the daemon's import job with an export in FERB's downloads folder. The import really
//! starts and records the file as FERB's to delete. This program has no model, so the run's chunk fails: the export
//! is kept (a part is unread; it would go a day later, MEM-440) and never deleted with parts unread. An export
//! outside FERB's downloads folder is refused by the tool itself.

mod shim;
mod support;

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use shim::started;
use support::*;

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn text(reply: &Value) -> String {
    reply["content"].to_string()
}

// decision: MEM-448, MEM-451, MEM-440
#[test]
fn memory_import_starts_the_real_import_and_keeps_an_export_with_parts_unread() {
    let tmp = root();
    let (run, mut shim) = started(tmp.path(), "01");
    let folder = run.home.join("workspace/downloads/a1-claude-export");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("conversations.json"),
        json!([{"uuid": "c1", "name": "About bees", "chat_messages": [
            {"sender": "human", "created_at": "2025-12-20T10:00:00Z",
             "content": [{"type": "text", "text": "I keep bees on the roof"}]}]}])
        .to_string(),
    )
    .unwrap();
    // outside FERB's downloads folder: refused before anything is read
    let elsewhere = run.user.join("Downloads/claude-export");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::copy(
        folder.join("conversations.json"),
        elsewhere.join("conversations.json"),
    )
    .unwrap();
    let refused = shim.call(
        "memory.import",
        json!({"source": "claude", "path": elsewhere.to_string_lossy(), "why": "the export the mail linked"}),
    );
    assert_eq!(refused["is_error"], true, "{refused}");
    assert!(
        text(&refused).contains("only an export FERB downloaded"),
        "{refused}"
    );
    assert_eq!(run.table("memory_import_runs"), json!([]));
    // FERB's own download: the real import starts in the background
    let reply = shim.call(
        "memory.import",
        json!({"source": "claude", "path": folder.to_string_lossy(), "why": "the export the mail linked"}),
    );
    assert_eq!(reply["is_error"], false, "{reply}");
    assert!(text(&reply).contains("\"run\""), "{reply}");
    until("the import ends", || {
        run.table("memory_import_runs")
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r[3] != "running")
    });
    let files = run.table("memory_import_files");
    assert_eq!(
        files.as_array().unwrap().len(),
        1,
        "the export is FERB's to delete: {files}"
    );
    // no model here: its one chunk failed, so the file stays for now (a part is unread)
    assert!(folder.join("conversations.json").exists());
    let runs = run.table("memory_import_runs");
    let counts: Value = serde_json::from_str(runs[0][7].as_str().unwrap()).unwrap();
    assert_eq!(counts["failed_chunks"], 1, "{counts}");
}
