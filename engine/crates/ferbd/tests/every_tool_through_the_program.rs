//! Every tool of the catalog through the real program: the `ferbd daemon` binary with the test stand-ins (no Mac parts,
//! no browser started, snapshots off), its composed registry and its gateway (audit of 2026-11-16, after fs.disk_usage
//! was found refused on every call: ferbd registered it without declaring its files, and the crate tests drove the
//! tool's own functions, never the tool as the daemon composes and gates it).
//!
//! For each tool of engine/fixtures/catalog/tools.json and ferb-gateway's catalog_additions.json:
//! 1. it is registered and isn't a "not available in this build" stub;
//! 2. what the gateway reads off it for a well-formed call is there: the files it names whenever it has an `fs:` scope
//!    or Python's tool had a `file_paths` method (the fs.disk_usage class), `real_path`, `recovery_reason` and
//!    `ledger_args` wherever Python's had them, its locks where Python's named one, and its describe kind is Python's;
//! 3. a well-formed call through `tools.call` (a run token's connection, the MCP shim's) passes every structural check
//!    (admission, the argument model, the file declaration) and reaches the tool; the ones that can run offline on a
//!    temp home return a result that is not an error.

mod shim;
mod support;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use shim::Shim;
use support::*;

/// What a call's reply must be.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Expect {
    /// It ran and returned a result that isn't an error.
    Works,
    /// It reached the tool, which refused for a reason of this test daemon (no browser connected, the Mac's parts
    /// off, nothing to stop): the text must contain this.
    Reaches(&'static str),
    /// Not called here: why (each is checked structurally above all the same).
    NotCalled(&'static str),
}
use Expect::*;

/// Replies that mean the call never reached the tool: a structural refusal of the gateway or a stub.
const STRUCTURAL: [&str; 7] = [
    "invalid arguments",
    "is not available to this worker",
    "not available to this task's identity",
    "doesn't declare the files it names",
    "not available in this build",
    "task is not running",
    "no such tool",
];

/// The Mac tools in a test daemon (stand-ins `mac` and `native` off) still read this Mac's own running and installed
/// apps before they refuse (mac.apps lists them): not called here (TEST-508); the Mac hands' fakes drive them in
/// ferb-mac-hands' tests, and the owner's live runs drove most of them (the audit's matrix says which).
const READS_THE_MAC: &str = "reads this Mac's own apps even in a test daemon (TEST-508)";

/// The supervisor's refusal of a front door call made outside a chat message's run (ORCH-736).
const NO_TURN: &str = "No chat message is being answered for this call.";

/// One well-formed call per tool, its expected reply, and Python's describe kind for it (a Rust addition's own).
fn calls() -> Vec<(&'static str, Value, Expect, &'static str)> {
    let no_tab = "no tab t1 open for this task";
    vec![
        ("actions.search", json!({"query": "files"}), Works, "read"),
        ("abilities.list", json!({}), Works, "read"),
        ("abilities.read", json!({"name": "google_docs"}), Works, "read"),
        ("device.facts", json!({}), Works, "read"),
        ("fs.list", json!({"path": "."}), Works, "read"),
        ("fs.read", json!({"path": "a.txt"}), Works, "read"),
        ("fs.look", json!({"path": "dot.png"}), Works, "read"),
        ("fs.disk_usage", json!({"path": "."}), Works, "read"),
        ("fs.write", json!({"path": "new.txt", "content": "made by the test\n"}), Works, "write_file"),
        ("fs.edit", json!({"path": "a.txt", "old_text": "hello", "new_text": "hullo"}), Works, "write_file"),
        ("fs.copy", json!({"path": "a.txt", "destination": "b.txt"}), Works, "write_file"),
        ("fs.move", json!({"path": "b.txt", "destination": "c.txt"}), Works, "write_file"),
        ("fs.trash", json!({"path": "c.txt"}), Works, "trash"),
        ("owner.ask", json!({"question": "Which one?", "options": ["A", "B"]}), NotCalled("answered by the gateway through the owner's chat: a test daemon has no owner to answer it"), "read"),
        ("recording.stop", json!({"id": "rec-none"}), Reaches("unknown recording id"), "write_file"),
        ("recording.inspect", json!({"path": "missing.mp4"}), Reaches("does not exist"), "read"),
        ("recovery.snapshot", json!({"before": "a test's change"}), Reaches("snapshots are turned off"), "snapshot"),
        ("browser.open", json!({"url": "https://example.com/"}), Reaches("no browser profile is paired"), "open_tab"),
        ("browser.navigate", json!({"tab": "t1", "url": "https://example.com/"}), Reaches(no_tab), "navigate"),
        ("browser.snapshot", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("browser.read", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("browser.html", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("browser.dialog", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("browser.profiles", json!({}), Works, "read"),
        ("browser.pair", json!({"profile": "chrome/Default"}), NotCalled("asks the owner first (pairing a profile)"), "click"),
        ("browser.handoff", json!({"tab": "t1", "message": "Please sign in"}), Reaches(no_tab), "read"),
        ("browser.screenshot", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("browser.pdf", json!({"tab": "t1", "path": "page.pdf"}), Reaches(no_tab), "write_file"),
        ("browser.point", json!({"tab": "t1", "shot": "s1", "x": 1, "y": 1}), Reaches(no_tab), "read"),
        ("browser.act", json!({"tab": "t1", "element": "e1", "action": "click"}), Reaches(no_tab), "click"),
        ("browser.wait", json!({"tab": "t1", "until": "load"}), Reaches(no_tab), "read"),
        ("browser.close", json!({"tab": "t1"}), Reaches(no_tab), "open_tab"),
        ("browser.keep_open", json!({"tab": "t1"}), Reaches(no_tab), "open_tab"),
        ("browser.scroll", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("browser.find", json!({"tab": "t1", "text": "Example"}), Reaches(no_tab), "read"),
        ("browser.downloads", json!({}), Works, "read"),
        ("browser.media", json!({"tab": "t1", "action": "pause"}), Reaches(no_tab), "read"),
        ("browser.tabs", json!({}), Works, "read"),
        ("browser.use_tab", json!({"tab_ref": "chrome/Default:11"}), Reaches("isn't a tab reference"), "open_tab"),
        ("browser.login_code", json!({"tab": "t1", "element": "e1"}), Reaches(no_tab), "fill_form"),
        ("browser.secret", json!({"tab": "t1", "element": "e1"}), Reaches(no_tab), "fill_form"),
        ("browser.record", json!({"tab": "t1", "path": "rec.mp4"}), Reaches(no_tab), "write_file"),
        ("mac.apps", json!({}), NotCalled(READS_THE_MAC), "read"),
        ("mac.open", json!({"bundle_id": "com.example.none"}), NotCalled(READS_THE_MAC), "open_tab"),
        ("mac.snapshot", json!({"app": "com.example.none"}), NotCalled(READS_THE_MAC), "read"),
        ("mac.menu", json!({"app": "com.example.none"}), NotCalled(READS_THE_MAC), "read"),
        ("mac.act", json!({"app": "com.example.none", "action": "press", "element": "e1"}), NotCalled(READS_THE_MAC), "mac_input"),
        ("mac.secret", json!({"app": "com.example.none", "element": "e1"}), NotCalled(READS_THE_MAC), "fill_form"),
        ("mac.applescript", json!({"app": "com.example.none", "script": "get name"}), NotCalled(READS_THE_MAC), "applescript"),
        ("mac.screenshot", json!({"app": "com.example.none"}), NotCalled(READS_THE_MAC), "read"),
        ("mac.click", json!({"app": "com.example.none", "shot": "s1", "x": 1, "y": 1}), NotCalled(READS_THE_MAC), "mac_input"),
        ("mac.pointer", json!({"app": "com.example.none", "shot": "s1", "x": 1, "y": 1, "action": "click"}), NotCalled(READS_THE_MAC), "mac_input"),
        ("mac.record", json!({"app": "com.example.none", "shot": "s1", "path": "rec.mp4"}), NotCalled(READS_THE_MAC), "write_file"),
        ("mac.clipboard", json!({"text": "invented"}), Reaches("without the Mac's own parts"), "mac_input"),
        ("audio.listen", json!({"seconds": 1, "app": "com.example.none"}), NotCalled(READS_THE_MAC), "read"),
        ("shell.run", json!({"command": "echo made-by-the-test"}), Works, "run_shell"),
        ("shell.start", json!({"command": "sleep 30"}), Works, "run_shell"),
        ("shell.wait", json!({"id": "$SHELL", "timeout_s": 1}), Works, "read"),
        ("shell.read", json!({"id": "$SHELL"}), Works, "read"),
        ("shell.send", json!({"id": "$SHELL", "text": "x\n"}), Works, "run_shell"),
        ("shell.list", json!({}), Works, "read"),
        ("shell.kill", json!({"id": "$SHELL"}), Works, "stop_process"),
        ("shell.rules", json!({}), Works, "read"),
        ("project.publish", json!({"folder": "/nonexistent-ferb-test-folder"}), Reaches("isn't a folder FERB can find"), "publish"),
        ("project.fetch", json!({"folder": "/nonexistent-ferb-test-folder"}), Reaches("isn't a folder FERB can find"), "fetch"),
        ("trail.query", json!({}), Works, "read"),
        ("trail.days", json!({}), Works, "read"),
        ("task.evidence", json!({}), Works, "read"),
        ("task.trace", json!({"task_id": "task-1"}), Reaches("isn't one of this conversation's tasks"), "read"),
        ("time.remind_at", json!({"in_minutes": 30, "message": "a test's reminder"}), Works, "timer"),
        ("time.set_timer", json!({"minutes": 30, "message": "a test's timer"}), Works, "timer"),
        ("time.sleep_until", json!({"in_minutes": 30}), NotCalled("sleeps until the fake clock reaches the time: the timer loop is off in a test daemon"), "read"),
        ("wait.for", json!({"any_of": [{"kind": "file", "path": "a.txt", "state": "exists"}]}), Works, "read"),
        ("skills.read", json!({"skill": "packing"}), Works, "read"),
        ("chat.history", json!({}), Works, "read"),
        // ORCH-736: the front door's tools, handed to the supervisor's front_call; task-1 is no chat message's run, so
        // the supervisor itself refuses each one (a well-formed call that reached the tool)
        ("chat.ask", json!({"question": "Which file?"}), Reaches(NO_TURN), "read"),
        ("chat.control_task", json!({"control": {"task_id": "task-1", "action": "stop", "new_goal": false,
            "standing_sends": "unchanged", "supersedes": []}}), Reaches(NO_TURN), "read"),
        // ORCH-743, ORCH-749: a hand-off carries its ack, goal and kind; a simple one the fields code writes its task from
        ("chat.dispatch_task", json!({"ack": "On it.", "goal": "Write hello into a.txt", "complexity": "simple",
            "quote": "write hello into a.txt", "name": "Hello file", "done_when": "a.txt holds hello", "identity": "owner",
            "answers_question": false}), Reaches(NO_TURN), "read"),
        ("chat.remember", json!({"forget": [{"item_id": "m-1", "quote": "forget that"}]}), Reaches(NO_TURN), "read"),
        ("chat.schedule", json!({"show_schedules": true}), Reaches(NO_TURN), "read"),
        ("chat.set_reminder", json!({"reminder_cancels": [{"id": "r-1", "quote": "cancel it"}]}), Reaches(NO_TURN), "read"),
        ("memory.read", json!({}), Works, "read"),
        ("secrets.list", json!({"place": "https://example.com/"}), Works, "read"),
        ("about.read", json!({}), Works, "read"),
        ("about.diagnostics", json!({}), Works, "read"),
        ("conversation.read", json!({}), Works, "read"),
        ("memory.learned", json!({}), Works, "read"),
        // MEM-448: an export path outside FERB's downloads folder, or none, is refused by the tool itself
        ("memory.import", json!({"source": "chatgpt", "path": "/nonexistent-ferb-export.zip"}), Reaches("doesn't exist"), "read"),
        ("ferb_browser.open", json!({"url": "https://example.com/"}), Reaches("isn't paired yet"), "open_tab"),
        ("ferb_browser.navigate", json!({"tab": "t1", "url": "https://example.com/"}), Reaches(no_tab), "navigate"),
        ("ferb_browser.snapshot", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("ferb_browser.read", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("ferb_browser.html", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("ferb_browser.dialog", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("ferb_browser.screenshot", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("ferb_browser.pdf", json!({"tab": "t1", "path": "page.pdf"}), Reaches(no_tab), "write_file"),
        ("ferb_browser.point", json!({"tab": "t1", "shot": "s1", "x": 1, "y": 1}), Reaches(no_tab), "read"),
        ("ferb_browser.act", json!({"tab": "t1", "element": "e1", "action": "click"}), Reaches(no_tab), "click"),
        ("ferb_browser.wait", json!({"tab": "t1", "until": "load"}), Reaches(no_tab), "read"),
        ("ferb_browser.close", json!({"tab": "t1"}), Reaches(no_tab), "open_tab"),
        ("ferb_browser.keep_open", json!({"tab": "t1"}), Reaches(no_tab), "open_tab"),
        ("ferb_browser.scroll", json!({"tab": "t1"}), Reaches(no_tab), "read"),
        ("ferb_browser.find", json!({"tab": "t1", "text": "Example"}), Reaches(no_tab), "read"),
        ("ferb_browser.downloads", json!({}), Works, "read"),
        ("ferb_browser.media", json!({"tab": "t1", "action": "pause"}), Reaches(no_tab), "read"),
        ("ferb_browser.fill_secret", json!({"tab": "t1", "element": "e1", "secret": "site-login"}), Reaches(no_tab), "fill_secret"),
        ("ferb_browser.fill_otp", json!({"tab": "t1", "element": "e1", "secret": "site-login", "candidate": "c1"}), Reaches(no_tab), "fill_secret"),
        ("ferb_browser.otp_candidates", json!({"tab": "t1", "element": "e1", "secret": "site-login"}), Reaches(no_tab), "read"),
        ("ferb_browser.record", json!({"tab": "t1", "path": "rec.mp4"}), Reaches(no_tab), "write_file"),
    ]
}

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Calls whose declarations Python's tools answered with the files named (rule 10), the describe kind with them:
/// a file a tool without an `fs:` scope names must reach the protection lists all the same.
fn declared_files() -> Vec<(&'static str, Value, Value, &'static str)> {
    vec![
        (
            "browser.act",
            json!({"tab": "t1", "element": "e1", "action": "upload", "files": ["a.txt"]}),
            json!([["a.txt", "read"]]),
            "",
        ),
        (
            "ferb_browser.act",
            json!({"tab": "t1", "element": "e1", "action": "upload", "files": ["a.txt"]}),
            json!([["a.txt", "read"]]),
            "",
        ),
        (
            "mac.open",
            json!({"bundle_id": "com.example.none", "path": "/tmp/fx-none/a.txt"}),
            json!([["/tmp/fx-none/a.txt", "read"]]),
            "open_tab",
        ),
        (
            "mac.applescript",
            json!({"app": "com.example.none",
            "script": "tell application \"Finder\" to delete POSIX file \"/tmp/fx-none/a.txt\""}),
            json!([["/tmp/fx-none/a.txt", "write"]]),
            "applescript",
        ),
        // wait.py:243: each watched file, read
        (
            "wait.for",
            json!({"any_of": [{"kind": "file", "path": "~/.ferb/config.toml", "state": "exists"}]}),
            json!([["~/.ferb/config.toml", "read"]]),
            "read",
        ),
        // wait.py:238: a wait with a script is a shell command for the gateway, as shell.run
        (
            "wait.for",
            json!({"any_of": [{"kind": "script", "command": "test -f done.txt"}]}),
            json!([]),
            "run_shell",
        ),
        (
            "fs.disk_usage",
            json!({"path": "~/Documents"}),
            json!([["~/Documents", "metadata"]]),
            "read",
        ),
    ]
}

fn catalog_tools() -> Vec<(String, Value)> {
    let catalog = fixture("catalog/tools.json");
    let mut all: Vec<(String, Value)> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["facts"].clone()))
        .collect();
    for a in ferb_gateway::catalog::additions() {
        all.push((a.name.clone(), a.row["facts"].clone()));
    }
    all
}

fn scopes(listing: &str) -> Value {
    fixture("catalog/tools.json")["listings"][listing]["grant"]["scopes"].clone()
}

/// A daemon with task-1 live, FERB's own profile's tools attached, an owner and a FERB token with every scope, a
/// planner token naming task.trace (ORCH-663) and a front door token naming its six tools (ORCH-736); the workspace
/// holds a.txt and the owner's skills folder one skill.
fn daemon(tmp: &std::path::Path) -> (Run, Shim, Shim, Shim, Shim) {
    let run = Run::start(tmp, "01", json!({"ids": "id"}));
    run.control(json!({"do": "attach_identity"}));
    let owner = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
        "identity": "owner", "scopes": scopes("worker_owner_all_scopes")}),
    );
    let ferb = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-2",
        "identity": "ferb", "scopes": scopes("worker_ferb_all_scopes")}),
    );
    let planner = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "planner-1",
        "identity": "owner", "scopes": ["task:read"], "tools": ["task.trace"]}),
    );
    let front = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "front-1",
        "identity": "owner", "scopes": ferb_orchestrator::lookups::FRONT_SCOPES,
        "tools": ferb_orchestrator::lookups::FRONT_TOOLS}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    let ws = run.home.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.txt"), "hello from the test\n").unwrap();
    std::fs::write(ws.join("dot.png"), PNG).unwrap();
    // the temp home's own Trash, where fs.trash moves the file (the real daemon's is the owner's ~/.Trash)
    std::fs::create_dir_all(run.user.join(".Trash")).unwrap();
    let skill = run.home.join("skills/packing");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "# Packing\nRoll, don't fold.\n").unwrap();
    let o = Shim::open(&run, owner.as_str().unwrap());
    let f = Shim::open(&run, ferb.as_str().unwrap());
    let p = Shim::open(&run, planner.as_str().unwrap());
    let d = Shim::open(&run, front.as_str().unwrap());
    (run, o, f, p, d)
}

/// A line of the audit's report, printed as it comes when FERB_AUDIT_REPORT is set.
fn note(report: &mut Vec<String>, line: String) {
    if std::env::var_os("FERB_AUDIT_REPORT").is_some() {
        eprintln!("{line}");
    }
    report.push(line);
}

/// The locks Python's tool takes for these arguments: a browser family's tab (tools.py:176), each file (files.py),
/// a recording (recording.py:502), the app (mac/tools.py:534); none for the rest of the table's calls.
fn python_locks(name: &str, args: &Value) -> Value {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let family = name.split('.').next().unwrap_or("");
    match name {
        "fs.list" | "fs.read" | "fs.look" | "fs.write" | "fs.edit" | "fs.trash" => {
            json!([format!("file:{}", s("path"))])
        }
        "fs.copy" | "fs.move" => json!([
            format!("file:{}", s("path")),
            format!("file:{}", s("destination"))
        ]),
        "recording.stop" => json!([format!("recording:{}", s("id"))]),
        "mac.menu" | "mac.act" | "mac.applescript" | "mac.click" | "mac.pointer" => {
            json!([format!("app:{}", s("app"))])
        }
        _ if (family == "browser" || family == "ferb_browser") && args.get("tab").is_some() => {
            json!([format!("tab:{family}/{}", s("tab"))])
        }
        _ => json!([]),
    }
}

fn method(facts: &Value, m: &str) -> bool {
    facts[m] == "<method>"
}

/// The whole check on one daemon, `before` run on it first; every problem found.
fn audit(before: impl Fn(&Run)) -> Vec<String> {
    let tmp = root();
    let (run, mut owner, mut ferb, mut planner, mut front) = daemon(tmp.path());
    before(&run);
    let table: BTreeMap<&str, (Value, Expect, &str)> = calls()
        .into_iter()
        .map(|(n, a, e, k)| (n, (a, e, k)))
        .collect();
    let mut problems: Vec<String> = Vec::new();
    let mut report: Vec<String> = Vec::new();

    let declared: BTreeSet<&str> = declared_files().iter().map(|(n, ..)| *n).collect();
    // 1 and 2: every catalog tool is registered, not a stub, and declares what Python's declared
    let all = catalog_tools();
    for (name, facts) in &all {
        let Some((args, _, kind)) = table.get(name.as_str()) else {
            problems.push(format!("{name}: no well-formed call in this test's table"));
            continue;
        };
        let d = run.control(json!({"do": "declare", "name": name, "args": args}));
        if d["registered"] != true {
            problems.push(format!("{name}: not registered"));
            continue;
        }
        if d["refusing"] == true {
            problems.push(format!("{name}: a \"not available in this build\" stub"));
        }
        let fs_scoped = d["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap().starts_with("fs:"));
        // the gateway refuses an fs-scoped tool that declares nothing (gateway.rs `file_rules`): fs.disk_usage
        if fs_scoped && d["file_paths"].is_null() {
            problems.push(format!(
                "{name}: has an fs scope and declares no file_paths"
            ));
        }
        // a tool without an fs scope whose Python tool named files: the files it names are checked below
        if !fs_scoped && method(facts, "file_paths") && !declared.contains(name.as_str()) {
            problems.push(format!(
                "{name}: Python's tool declared files; add a call naming one to declared_files()"
            ));
        }
        let want = python_locks(name, args);
        if d["locks"] != want {
            problems.push(format!("{name}: locks {} (Python's: {want})", d["locks"]));
        }
        for m in ["real_path", "recovery_reason", "ledger_args"] {
            if method(facts, m) && d[m] != true {
                problems.push(format!("{name}: no {m}, which Python's tool had"));
            }
        }
        if d["kind"] != *kind {
            problems.push(format!(
                "{name}: describe kind {} (Python's: {kind})",
                d["kind"]
            ));
        }
        note(&mut report, format!("{name}: {d}"));
    }
    for (name, args, files, kind) in declared_files() {
        let d = run.control(json!({"do": "declare", "name": name, "args": args}));
        if d["file_paths"] != files {
            problems.push(format!(
                "{name}: declares {} for {args} (Python's: {files})",
                d["file_paths"]
            ));
        }
        if !kind.is_empty() && d["kind"] != kind {
            problems.push(format!(
                "{name}: describe kind {} for {args} (Python's: {kind})",
                d["kind"]
            ));
        }
        note(&mut report, format!("{name} {args}: {d}"));
    }
    // and nothing in the table that the catalog doesn't have
    let names: BTreeSet<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    for n in table.keys() {
        if !names.contains(n) {
            problems.push(format!("{n}: in the table, not in the catalog"));
        }
    }

    // 3: each call through the gateway
    let mut shell_id = String::new();
    for (name, args, expect, _) in calls() {
        if let NotCalled(_) = expect {
            continue;
        }
        let mut args = args;
        if args["id"] == "$SHELL" {
            args["id"] = json!(shell_id);
        }
        let shim = if name.starts_with("ferb_browser.") {
            &mut ferb
        } else if name == "task.trace" {
            &mut planner
        } else if ferb_orchestrator::lookups::FRONT_TOOLS.contains(&name) {
            &mut front
        } else {
            &mut owner
        };
        let got = shim.call(name, args);
        let text = got["content"].to_string();
        note(&mut report, format!("CALL {name}: {got}"));
        if let Some(s) = STRUCTURAL.iter().find(|s| text.contains(**s)) {
            problems.push(format!(
                "{name}: refused before reaching the tool ({s}): {text}"
            ));
            continue;
        }
        match expect {
            Works if got["is_error"] != false => {
                problems.push(format!("{name}: expected a result, got an error: {text}"))
            }
            Reaches(why) if got["is_error"] != true || !text.contains(why) => problems.push(
                format!("{name}: expected the tool's own refusal ({why}): {got}"),
            ),
            _ => {}
        }
        if name == "shell.start" {
            shell_id = got["content"]["id"].as_str().unwrap_or("").to_string();
        }
    }
    problems
}

// decision: ENG-19
// decision: ENG-20
#[test]
fn every_catalog_tool_is_wired_and_reaches_its_work() {
    let problems = audit(|_| {});
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The check fails on the bug it was written for: fs.disk_usage composed with only a describe and a run (as it was
/// before 2026-11-16) is flagged three times: no files declared for either call, and refused by the gateway on a well-formed call.
// decision: ENG-19
#[test]
fn the_check_catches_a_tool_that_declares_no_files() {
    let problems = audit(|run| {
        run.control(json!({"do": "undeclare", "name": "fs.disk_usage"}));
    });
    let want = [
        "fs.disk_usage: has an fs scope and declares no file_paths",
        "fs.disk_usage: declares null for {\"path\":\"~/Documents\"}",
        "fs.disk_usage: refused before reaching the tool (doesn't declare the files it names)",
    ];
    assert_eq!(problems.len(), want.len(), "{problems:#?}");
    for (got, want) in problems.iter().zip(want) {
        assert!(got.starts_with(want), "{got}");
    }
}
