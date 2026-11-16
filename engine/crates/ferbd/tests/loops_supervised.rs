//! T-1123: the daemon's background loops are supervised as main.py's `_supervise` (compose/guard.rs). A loop that
//! fails is journaled `daemon.loop_failed` and starts again; `about.diagnostics` reads the row; and no loop or one-off
//! job of the composition is spawned outside the guard (the source scan below, with `spawns.txt`).

mod support;

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::*;

fn rpc(c: &mut Raw, id: i64, method: &str, params: Value) -> Value {
    c.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
    let line = c.recv().expect("an answer");
    serde_json::from_str(&line).expect("JSON")
}

fn loop_failed(run: &Run) -> Vec<Value> {
    let db = rusqlite::Connection::open(run.home.join("ferb.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let mut q = db
        .prepare("SELECT data_json FROM journal WHERE kind = 'daemon.loop_failed' ORDER BY seq")
        .unwrap();
    let rows: Vec<String> = q
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.iter()
        .map(|d| serde_json::from_str(d).unwrap())
        .collect()
}

/// The trail's thread (sensing's trail and perception, `loops.rs trail_thread`) panics in the middle of its work: the
/// failure is journaled with the loop's name, the thread starts again after Python's first pause (1 s, asked of the
/// stand-ins' pause, which doesn't wait), and perception answers `sense.status` again.
// decision: DMN-599
#[test]
fn a_loop_that_panics_is_journaled_and_started_again() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"group_a": true}));
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    let before = rpc(&mut c, 1, "sense.status", json!({}));
    assert!(before["result"]["perception"].is_object(), "{before}");
    run.control(json!({"do": "panic_trail"}));
    let until = Instant::now() + Duration::from_secs(10);
    let (rows, after) = loop {
        let rows = loop_failed(&run);
        let after = rpc(&mut c, 2, "sense.status", json!({}));
        if (!rows.is_empty() && after["result"]["perception"].is_object())
            || Instant::now() >= until
        {
            break (rows, after);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        rows.len(),
        1,
        "one daemon.loop_failed row: {rows:?}; sense.status then: {after}"
    );
    assert_eq!(rows[0]["loop"], "trail");
    assert_eq!(rows[0]["failures"], 1);
    assert!(
        rows[0]["error"].as_str().unwrap().starts_with("panic: "),
        "{rows:?}"
    );
    assert!(
        after["result"]["perception"].is_object(),
        "the trail's thread runs again: {after}"
    );
    assert_eq!(run.control(json!({"do": "loop_pauses"})), json!([1.0]));
    // the planner's self-diagnosis (`about.diagnostics`) reads it: one of FERB's own journal kinds
    let seen = ferbd::compose::tools::diagnostics::journal(
        &run.home.join("ferb.db"),
        "2026-01-01T00:00:00+00:00",
        Some("daemon.loop_failed"),
    )
    .unwrap();
    assert_eq!(seen["total"], 1, "{seen}");
    assert_eq!(seen["records"][0]["kind"], "daemon.loop_failed", "{seen}");
}

/// The spawns of src/compose (each file up to its test module), as (file, key): a `std::thread::Builder` chain is one
/// spawn keyed by its thread's name; any other spawn by its trimmed line. compose/guard.rs is the guard itself.
fn spawns() -> Vec<(String, String, usize)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    let mut dirs = vec![root.join("src/compose")];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("compose/guard.rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let rel = f.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        let lines: Vec<String> = std::fs::read_to_string(&f)
            .unwrap()
            .lines()
            .take_while(|l| !l.trim_start().starts_with("#[cfg(test)]"))
            .map(str::to_string)
            .collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            if t.starts_with("//") || t.contains("spawn_blocking") {
                continue;
            }
            if t.contains("thread::Builder::new()") {
                let name = lines[i..lines.len().min(i + 3)]
                    .iter()
                    .find_map(|l| l.split(".name(\"").nth(1))
                    .and_then(|r| r.split('"').next())
                    .unwrap_or("(unnamed)");
                out.push((rel.clone(), format!("thread \"{name}\""), i + 1));
                continue;
            }
            let chained = i > 0
                && lines[i.saturating_sub(3)..i]
                    .iter()
                    .any(|l| l.contains("thread::Builder::new()"));
            if chained && t.starts_with(".spawn(") {
                continue;
            }
            if t.contains("tokio::spawn(")
                || t.contains("thread::spawn(")
                || t.contains("spawn_local(")
                || t.contains(".spawn(")
            {
                out.push((rel.clone(), t.to_string(), i + 1));
            }
        }
    }
    out
}

/// Every spawn of the composition goes through compose/guard.rs or is per-call work listed in `spawns.txt` with why;
/// a line of `spawns.txt` that no spawn matches any more is refused too.
// decision: DMN-599
#[test]
fn every_loop_of_the_composition_is_supervised() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("spawns.txt")).unwrap();
    let allowed: Vec<(String, String)> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let parts: Vec<&str> = l.splitn(3, " | ").collect();
            assert_eq!(parts.len(), 3, "file | spawn | why: {l}");
            assert!(!parts[2].trim().is_empty(), "no why: {l}");
            (parts[0].trim().to_string(), parts[1].trim().to_string())
        })
        .collect();
    let mut used = vec![false; allowed.len()];
    let mut unlisted = Vec::new();
    for (file, key, line) in spawns() {
        match allowed
            .iter()
            .position(|(f, k)| *f == file && (key == *k || key.contains(k.as_str())))
        {
            Some(i) => used[i] = true,
            None => unlisted.push(format!("{file}:{line}: {key}")),
        }
    }
    assert!(
        unlisted.is_empty(),
        "spawned outside compose/guard.rs (supervise it, or list per-call work in spawns.txt):\n{}",
        unlisted.join("\n")
    );
    let stale: Vec<_> = allowed
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|(a, _)| format!("{} | {}", a.0, a.1))
        .collect();
    assert!(
        stale.is_empty(),
        "spawns.txt lines nothing matches:\n{}",
        stale.join("\n")
    );
}
