//! `areas_c` through the real `ferbd daemon` program: every line rust/fixtures/daemon/rpc_transcripts.json recorded
//! from the Python daemon for the task records, permissions and health, FERB.app's own calls, the delegated
//! conversations and `logs.subscribe`, replayed and compared byte for byte, or as the named fix (ENG-6) or named
//! difference says. The recorder's invented Mac comes through the test build's stand-ins (`"group_c": "recorder"`).

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use support::*;

const C: [&str; 20] = [
    "doctor.check",
    "setup.permissions",
    "setup.permission_request",
    "setup.finish",
    "clipboard.request",
    "automation.request",
    "ui.click_through",
    "identity.check",
    "conversations.list",
    "conversations.change",
    "tasks.preview",
    "questions.picture",
    "reports.build",
    "reports.diagnostic",
    "reports.send",
    "recovery.list",
    "recovery.restore",
    "trace.get",
    "verdict.set",
    "logs.subscribe",
];

/// Named fixes (ENG-6): Python answered these bad parameters with an internal error (-32603 and the exception's
/// class); Rust refuses (-32004) in plain words.
const FIXED: &[(&str, i64, &str, &str)] = &[
    (
        "settings_setup_about",
        21,
        "TypeError",
        "bundle_ids must be a list of app bundle ids",
    ),
    (
        "settings_setup_about",
        58,
        "ValueError",
        "No such delegated conversation",
    ),
    (
        "settings_setup_about",
        59,
        "ValueError",
        "No such delegated conversation",
    ),
    (
        "settings_setup_about",
        60,
        "KeyError",
        "conversations.change needs the conversation's id, an action and the owner's words",
    ),
];

/// Values that change between runs (the recorder's own `VOLATILE_KEYS` and the store's size).
const VOLATILE: [&str; 5] = ["pid", "executable", "code_hash", "grant_target", "bytes"];

fn scrub(line: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    fn walk(v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if VOLATILE.contains(&k.as_str()) {
                        *x = Value::String("<volatile>".into());
                    } else {
                        walk(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    ferb_py::json::dumps(&v)
}

/// The recording's `/FX/<nn>/home` as this run's home.
fn localize(text: &str, home: &Path) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("/FX/") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 4..];
        let b = tail.as_bytes();
        if b.len() >= 7 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && &tail[2..7] == "/home"
        {
            out.push_str(&home.to_string_lossy());
            rest = &tail[7..];
        } else {
            out.push_str("/FX/");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The named differences of `doctor.check`: the Rust build carries o200k_base (the recorder's Python lacked its
/// file), and the sandbox is really checked here (the recorder stubbed `Sandbox.available` to say "available"; the
/// real check says "ok", in Python as in Rust).
fn tokenizer(want: &str) -> String {
    want.replace(
        "\"tokenizer\": \"utf8_byte_ceiling_tokenizer_unavailable\"",
        "\"tokenizer\": \"o200k_base_chunked8192_estimate_provider_may_differ\"",
    )
    .replace(
        "\"reason\": \"available\", \"shell\": true, \"shell_reason\": \"available\"",
        "\"reason\": \"ok\", \"shell\": true, \"shell_reason\": \"ok\"",
    )
    // the runtimes' stand-in (b_runtimes.rs `World`, as the runtimes replay uses it) has no Claude program
    .replace(
        "\"claude\": \"0.2.174 (Claude Code 2.1.310)\"",
        "\"claude\": \"\"",
    )
}

struct Tally {
    same: BTreeMap<String, usize>,
    fixed: usize,
    diffs: Vec<String>,
}

/// Replays the scenario's calls and streams of `areas_c`'s methods (and their connections' hellos); `before`: a
/// control request made just before the request with that id on that connection.
fn replay(name: &str, run: &Run, before: &[(&str, i64, Value)]) -> Tally {
    replay_with(name, run, before, &[], &|w| w)
}

/// As [`replay`]; `also`: methods sent too (their answers read, not compared); `want`: a named difference applied to
/// each recorded line.
fn replay_with(
    name: &str,
    run: &Run,
    before: &[(&str, i64, Value)],
    also: &[&str],
    want_as: &dyn Fn(String) -> String,
) -> Tally {
    let scenario = scenario(&fixture("daemon/rpc_transcripts.json"), name);
    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    // a stream's connection the client closed: its lines are still read (they were recorded as they came)
    let mut cut: BTreeMap<String, Raw> = BTreeMap::new();
    let mut streams: BTreeMap<String, String> = BTreeMap::new();
    let mut t = Tally {
        same: BTreeMap::new(),
        fixed: 0,
        diffs: Vec::new(),
    };
    let check = |t: &mut Tally, c: &mut Raw, method: &str, id: Option<i64>, want: String| {
        let want = want_as(tokenizer(&scrub(&localize(&want, &run.home))));
        let got = c.recv().map(|g| scrub(&g));
        let fix = FIXED
            .iter()
            .find(|(s, i, _, _)| *s == name && Some(*i) == id);
        if let Some((_, i, class, words)) = fix {
            let python: Value = serde_json::from_str(&want).unwrap();
            assert_eq!(
                python["error"]["code"], -32603,
                "{name} {i}: Python's answer"
            );
            assert_eq!(
                python["error"]["message"], *class,
                "{name} {i}: Python's class"
            );
            let rust = ferb_py::json::dumps(
                &json!({"jsonrpc": "2.0", "id": i, "error": {"code": -32004, "message": words}}),
            );
            if got.as_deref() == Some(rust.as_str()) {
                t.fixed += 1;
            } else {
                t.diffs.push(format!(
                    "{method} (fixed):\n  want: {rust}\n  rust: {got:?}"
                ));
            }
            return;
        }
        if got.as_deref() == Some(want.as_str()) {
            *t.same.entry(method.to_string()).or_default() += 1;
        } else {
            t.diffs
                .push(format!("{method}:\n  python: {want}\n  rust:   {got:?}"));
        }
    };
    for st in scenario["steps"].as_array().unwrap() {
        let label = st["conn"].as_str().unwrap_or("").to_owned();
        match st["do"].as_str().unwrap_or("") {
            "connect" => {
                conns.insert(label, Raw::connect(&run.socket()).unwrap());
            }
            "close" => {
                if let Some(c) = conns.remove(&label) {
                    if streams.contains_key(&label) {
                        cut.insert(label, c);
                    }
                }
            }
            "call" => {
                let lines = st["lines"].as_array().unwrap();
                let Some(first) = lines.first().and_then(|l| l.get("send")) else {
                    continue;
                };
                let request = localize(&line_text(first), &run.home);
                let (method, id) = method_of(&request);
                let other = also.contains(&method.as_str());
                if method != "auth.hello" && !C.contains(&method.as_str()) && !other {
                    continue;
                }
                for (conn, at, req) in before {
                    if *conn == label && id == Some(*at) {
                        run.control(req.clone());
                    }
                }
                let c = conns.get_mut(&label).unwrap();
                c.send(&request);
                for l in &lines[1..] {
                    if l.get("recv").is_some() {
                        if other {
                            c.recv().expect("an answer");
                        } else {
                            check(&mut t, c, &method, id, line_text(&l["recv"]));
                        }
                    }
                }
            }
            "stream" => {
                let request = localize(&line_text(&st["lines"][0]["send"]), &run.home);
                let (method, _) = method_of(&request);
                if !C.contains(&method.as_str()) {
                    continue;
                }
                conns.get_mut(&label).unwrap().send(&request);
                streams.insert(label, method);
            }
            "stream_end" | "stream_cut" => {
                let Some(method) = streams.remove(&label) else {
                    continue;
                };
                let mut kept = cut.remove(&label);
                let c = match kept.as_mut() {
                    Some(c) => c,
                    None => match conns.get_mut(&label) {
                        Some(c) => c,
                        None => continue,
                    },
                };
                for l in st["lines"].as_array().unwrap() {
                    if let Some(r) = l.get("recv") {
                        check(&mut t, c, &method, None, line_text(r));
                    }
                }
            }
            _ => {}
        }
    }
    t
}

fn assert_clean(name: &str, t: &Tally) {
    assert!(
        t.diffs.is_empty(),
        "{name}: {} difference(s):\n{}",
        t.diffs.len(),
        t.diffs.join("\n")
    );
    eprintln!("{name}: same {:?}, fixed {}", t.same, t.fixed);
}

#[test]
fn settings_setup_and_health_through_the_binary() {
    let tmp = root();
    let run = Run::start(
        tmp.path(),
        "30",
        json!({"ids": "id", "group_a": true, "programs": "recorder", "group_c": "recorder"}),
    );
    let t = replay("settings_setup_about", &run, &[]);
    assert_clean("settings_setup_about", &t);
    assert_eq!(t.fixed, 4);
    for m in [
        "doctor.check",
        "setup.permissions",
        "setup.permission_request",
        "setup.finish",
        "clipboard.request",
        "automation.request",
        "ui.click_through",
        "identity.check",
        "conversations.list",
        "tasks.preview",
        "questions.picture",
        "reports.diagnostic",
    ] {
        assert!(t.same.contains_key(m), "{m} not compared: {:?}", t.same);
    }
}

#[test]
fn logs_subscribe_through_the_binary() {
    let tmp = root();
    let run = Run::start(tmp.path(), "40", json!({"ids": "id", "group_a": true}));
    // the global stop's two hooks (the supervisor's runtimes, FERB's own browser) as Python's
    let t = replay_with(
        "streams_and_processes",
        &run,
        &[],
        &["stop.engage", "stop.lift"],
        &|w| w,
    );
    assert_clean("streams_and_processes", &t);
    // the filtered stream's two events and the refused filter
    assert_eq!(t.same.get("logs.subscribe"), Some(&3), "{:?}", t.same);
}

/// make.py `embed_refs`: `"@@body:<sha256>:ascii@@"` / `:utf8@@` stand for that body as `json.dumps` writes it
/// (quotes included), with or without `ensure_ascii`.
fn bodies(cell: &str) -> String {
    let mut out = String::new();
    let mut rest = cell;
    while let Some(i) = rest.find("\"@@body:") {
        let tail = &rest[i + 8..];
        let Some(end) = tail.find("@@\"") else { break };
        let (key, form) = tail[..end].split_once(':').unwrap();
        let text = line_text(&json!({"sha256": key}));
        out.push_str(&rest[..i]);
        out.push_str(&if form == "ascii" {
            ferb_py::json::dumps(&json!(text))
        } else {
            ferb_py::json::dumps_utf8(&json!(text))
        });
        rest = &tail[end + 3..];
    }
    out.push_str(rest);
    out
}

/// The scenario's end tables as INSERTs (each big text from bodies.json; a journal row's id and order its own).
fn seed(name: &str, tables: &[&str], home: &Path) -> Value {
    let scenario = scenario(&fixture("daemon/rpc_transcripts.json"), name);
    let mut out = Vec::new();
    for t in tables {
        let table = &scenario["tables"][*t];
        let columns: Vec<String> = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let skip_seq = *t == "journal";
        let keep: Vec<usize> = (0..columns.len())
            .filter(|i| !(skip_seq && columns[*i] == "seq"))
            .collect();
        let names: Vec<String> = keep
            .iter()
            .map(|i| format!("\"{}\"", columns[*i]))
            .collect();
        let marks = vec!["?"; keep.len()].join(", ");
        let sql = format!(
            "INSERT OR REPLACE INTO \"{t}\" ({}) VALUES ({marks})",
            names.join(", ")
        );
        for row in table["rows"].as_array().unwrap() {
            let mut cells: Vec<Value> = keep
                .iter()
                .map(|i| {
                    let v = &row[*i];
                    let v = if v.get("sha256").is_some() {
                        Value::String(line_text(v))
                    } else {
                        v.clone()
                    };
                    match v {
                        Value::String(s) => Value::String(bodies(&localize(&s, home))),
                        other => other,
                    }
                })
                .collect();
            // a journal row replaces the start's own row that took its id (the ids are the recording's)
            if skip_seq {
                cells.truncate(keep.len());
            }
            out.push(json!([sql, cells]));
        }
    }
    json!(out)
}

#[test]
fn task_records_through_the_binary() {
    let tmp = root();
    // the root as it resolves (/tmp is a link to /private/tmp): a restore answers the resolved path, as Python's
    let resolved = tmp.path().canonicalize().unwrap();
    let run = Run::start(
        &resolved,
        "35",
        json!({"ids": "id", "group_a": true, "programs": "recorder", "group_c": "recorder"}),
    );
    // the store and files as the recorded tasks left them: list.md written by the first, FERB's copy from before
    let ws = run.home.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("list.md"), "- milk\n- bread\n").unwrap();
    let kept = run.home.join("recovery/2026-01-01");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(kept.join("id-000033.md"), "- milk\n").unwrap();
    let statements = seed(
        "tasks_and_records",
        &[
            "conversations",
            "tasks",
            "task_results",
            "task_result_names",
            "task_context",
            "task_plans",
            "plan_steps",
            "spans",
            "episodes",
            "recovery",
            "action_ledger",
            "journal",
        ],
        &run.home,
    );
    run.control(json!({"do": "sql", "statements": statements}));
    let t = replay(
        "tasks_and_records",
        &run,
        &[("app#3", 7, json!({"do": "ids_next", "n": 81}))],
    );
    assert_clean("tasks_and_records", &t);
    for m in [
        "trace.get",
        "verdict.set",
        "reports.build",
        "reports.send",
        "recovery.list",
        "recovery.restore",
    ] {
        assert!(t.same.contains_key(m), "{m} not compared: {:?}", t.same);
    }
    // the verdict as recorded, and the file put back
    let want = scenario(&fixture("daemon/rpc_transcripts.json"), "tasks_and_records")["tables"]
        ["owner_verdicts"]["rows"]
        .clone();
    assert_eq!(run.table("owner_verdicts"), want);
    assert_eq!(
        std::fs::read_to_string(ws.join("list.md")).unwrap(),
        "- milk\n"
    );
}
