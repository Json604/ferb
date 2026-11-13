//! Replays `rust/fixtures/learning` (made by running the Python code, ENG-3) through the Rust port and reports every
//! difference: what each step returned or raised, every event told, and every table it changed.

mod support;

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use ferb_learning::{py, sql, text};
use support::*;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

fn outcome(r: ferb_learning::Result<Value>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"raises": e.class(), "message": e.to_string()}),
    }
}

/// Floats compare as Python's repr would show them (17 significant digits are the same double).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            x.as_f64() == y.as_f64() && (x.is_f64() || y.is_f64() || x == y)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn short(v: &Value) -> String {
    let t = v.to_string();
    if t.chars().count() > 1500 {
        format!("{}…", t.chars().take(1500).collect::<String>())
    } else {
        t
    }
}

fn changed(before: &Map<String, Value>, after: &Map<String, Value>) -> Map<String, Value> {
    after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Runs one recorded scenario; returns (steps compared, differences).
fn replay(sc: &Value, root: &std::path::Path) -> (usize, Vec<String>) {
    let name = sc["name"].as_str().unwrap();
    if let Some(export) = sc.get("export") {
        let folder = root.join(name).join("export");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("conversations.json"), export.to_string()).unwrap();
    }
    let env = Env::new(name, Some(root));
    let runtime = rt();
    let mut diffs = Vec::new();
    let mut before = hide_root(Value::Object(dump(&env)), root)
        .as_object()
        .unwrap()
        .clone();
    let steps = sc["steps"].as_array().unwrap();
    for (n, step) in steps.iter().enumerate() {
        let op_name = step["op"].as_str().unwrap();
        let args: Value = serde_json::from_str(
            &step["args"]
                .to_string()
                .replace("/FX", &root.to_string_lossy()),
        )
        .unwrap();
        // the answers this step's model calls gave, in order
        let mut answers = Vec::new();
        let evs = step["events"].as_array().unwrap();
        for (i, e) in evs.iter().enumerate() {
            if e[0] == "make_runtime" {
                let started = evs[i + 1..]
                    .iter()
                    .take_while(|x| x[0] != "make_runtime")
                    .find(|x| x[0] == "model.start")
                    .map(|x| x[1]["answer"].clone());
                answers.push(started.unwrap_or_else(|| json!({"tight": true, "answer": {}})));
            }
        }
        *env.answers.borrow_mut() = answers.into();
        env.events.lock().unwrap().clear();
        let got = hide_root(outcome(runtime.block_on(op(&env, op_name, &args))), root);
        let events = hide_root(Value::Array(env.events.lock().unwrap().clone()), root);
        let after = hide_root(Value::Object(dump(&env)), root)
            .as_object()
            .unwrap()
            .clone();
        let tables = changed(&before, &after);
        before = after;
        let at = format!("{name} step {n} ({op_name} {})", short(&args));
        if !same(&got, &step["out"]) {
            diffs.push(format!(
                "{at}: returned\n  rust:   {}\n  python: {}",
                short(&got),
                short(&step["out"])
            ));
        }
        if !same(&events, &step["events"]) {
            let (r, p) = (
                events.as_array().unwrap(),
                step["events"].as_array().unwrap(),
            );
            let first = (0..r.len().max(p.len()))
                .find(|&i| r.get(i).zip(p.get(i)).is_none_or(|(a, b)| !same(a, b)));
            let i = first.unwrap_or(0);
            diffs.push(format!(
                "{at}: events differ at {i} ({} rust, {} python)\n  rust:   {}\n  python: {}",
                r.len(),
                p.len(),
                r.get(i).map_or_else(String::new, short),
                p.get(i).map_or_else(String::new, short)
            ));
        }
        let want = step["tables"].as_object().unwrap();
        for t in want.keys().chain(tables.keys()).collect::<BTreeSet<_>>() {
            if !tables
                .get(t)
                .zip(want.get(t))
                .is_some_and(|(a, b)| same(a, b))
            {
                diffs.push(format!(
                    "{at}: table {t}\n  rust:   {}\n  python: {}",
                    tables.get(t).map_or_else(|| "(unchanged)".into(), short),
                    want.get(t).map_or_else(|| "(unchanged)".into(), short)
                ));
            }
        }
    }
    (steps.len(), diffs)
}

fn report(what: &str, n: usize, diffs: &[String]) {
    eprintln!("{what}: {n} compared, {} differ", diffs.len());
    for d in diffs.iter().take(12) {
        eprintln!("{d}\n");
    }
}

// decision: MEM-2, MEM-10, MEM-11, MEM-12, MEM-14, MEM-21, MEM-23, MEM-32, MEM-34
#[test]
fn memory_items_and_forget() {
    run_scenarios(&["items", "forget", "journal", "upgrade"]);
}

// decision: MEM-5, MEM-7, MEM-17, MEM-20, MEM-35, MEM-423, MEM-424, MEM-425, MEM-426, MEM-427, MEM-429
#[test]
fn core_profile_and_mac_scan() {
    run_scenarios(&["profile", "profile_seed", "mac"]);
}

// decision: MEM-1, MEM-3, MEM-29, MEM-30, MEM-31, MEM-422
#[test]
fn learned_nodes_cards_and_playbooks() {
    run_scenarios(&[
        "learning",
        "learning_owner",
        "cards",
        "cleanout",
        "playbooks",
    ]);
}

// decision: MEM-8, MEM-22, MEM-421
#[test]
fn daily_summary_and_distill() {
    run_scenarios(&["summary", "distill"]);
}

// decision: MEM-400, MEM-402, MEM-406, MEM-409, MEM-410, MEM-411, MEM-412
#[test]
fn memory_import_runs() {
    let data = fixture("scenarios.json");
    let tmp = tempfile::tempdir().unwrap();
    let (mut total, mut all) = (0, Vec::new());
    for sc in data["imports"].as_array().unwrap() {
        let (n, d) = replay(sc, tmp.path());
        total += n;
        all.extend(d);
    }
    report("imports", total, &all);
    assert!(all.is_empty());
}

fn run_scenarios(names: &[&str]) {
    let data = fixture("scenarios.json");
    let tmp = tempfile::tempdir().unwrap();
    let (mut total, mut all) = (0, Vec::new());
    for sc in data["scenarios"].as_array().unwrap() {
        if names.contains(&sc["name"].as_str().unwrap()) {
            let (n, d) = replay(sc, tmp.path());
            total += n;
            all.extend(d);
        }
    }
    report(&names.join(", "), total, &all);
    assert!(all.is_empty());
}

fn sorted_rows(v: Vec<Value>) -> Vec<Value> {
    let mut v = v;
    v.sort_by_key(|x| x.to_string());
    v
}

/// make.py's `normalized`: what a restart run must end with, ids aside.
fn normalized(env: &Env) -> Value {
    let q = |s: &str| env.store.query(s, sql![]).unwrap();
    let facts = sorted_rows(q("SELECT kind, subject, key, content, origin, status, valid_to IS NULL FROM memory_items")
        .iter()
        .map(|r| Value::Array(r.json()))
        .collect());
    let done = sorted_rows(
        q("SELECT source, part FROM memory_import_done")
            .iter()
            .map(|r| Value::Array(r.json()))
            .collect(),
    );
    let runs: Vec<Value> = q("SELECT status, counts_json FROM memory_import_runs")
        .iter()
        .map(|r| {
            let mut c: Value = serde_json::from_str(&r.s_at(1)).unwrap();
            c.as_object_mut().unwrap().remove("saved_ids");
            json!([r.s_at(0), c])
        })
        .collect();
    let said = sorted_rows(q("SELECT m.content, i.source, i.said_at FROM memory_import_items i JOIN memory_items m ON m.id = i.item_id")
        .iter()
        .map(|r| Value::Array(r.json()))
        .collect());
    let mut kinds: Vec<String> = q("SELECT kind FROM journal")
        .iter()
        .map(|r| r.s_at(0))
        .collect();
    kinds.sort();
    // a resumed run writes its own source row (`_source_entry`), as Python does on every resume
    kinds.dedup_by(|a, b| a == b && a == "memory.import");
    json!({"facts": facts, "done": done, "runs": runs, "import_items": said, "journal_kinds": kinds})
}

fn normalize_fixture(v: &Value) -> Value {
    let mut v = v.clone();
    for k in ["facts", "done", "import_items"] {
        v[k] = Value::Array(sorted_rows(v[k].as_array().unwrap().clone()));
    }
    let mut kinds: Vec<Value> = v["journal_kinds"].as_array().unwrap().clone();
    kinds.dedup_by(|a, b| a == b && *a == "memory.import");
    v["journal_kinds"] = Value::Array(kinds);
    v
}

/// decision: MEM-410. Fixed against Python (ENG-6, `import_job.py:186-190`): a chunk's facts, its source row, its
/// done marks and the run's progress commit together, and a run's closing writes too. Python's own runs, failing at
/// each write and started again, are recorded beside the clean run (`restarts.json` `runs`); most end with work
/// lost or counted twice. The Rust run, failing at each of its writes and started again, must end exactly as the
/// clean Python run did: that is the one deliberate difference.
#[test]
fn import_restart_at_every_write_neither_loses_nor_doubles() {
    let data = fixture("restarts.json");
    let imp = &data["import"];
    let chunk_chars = imp["chunk_chars"].as_u64().unwrap() as usize;
    let importer = |env: &Env| {
        let mut job = env.importer();
        job.chunk_chars = chunk_chars;
        job
    };
    let tmp = tempfile::tempdir().unwrap();
    let start = |name: &str| {
        let folder = tmp.path().join(name).join("export");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("conversations.json"), imp["export"].to_string()).unwrap();
        let env = Env::new(name, Some(tmp.path()));
        *env.clock.0.borrow_mut() = py::parse("2026-10-25T09:00:00+00:00").unwrap();
        *env.answers.borrow_mut() = vec![json!({"dynamic": "import"}); 400].into();
        let path = folder.to_string_lossy().to_string();
        importer(&env).create("chatgpt", &path).unwrap();
        env
    };
    let runtime = rt();
    let clean_py = normalize_fixture(&imp["clean"]);
    let env = start("imp-count");
    let before = env.writes.get();
    runtime.block_on(importer(&env).run("id-000001")).unwrap();
    let total = env.writes.get() - before;
    let clean = normalized(&env);
    let mut diffs = Vec::new();
    if !same(&clean, &clean_py) {
        diffs.push(format!(
            "clean run\n  rust:   {}\n  python: {}",
            short(&clean),
            short(&clean_py)
        ));
    }
    let python_lost = imp["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| normalize_fixture(&r["state"]) != clean_py)
        .count();
    for k in 1..=total {
        let env = start(&format!("imp-{k}"));
        env.fail_at.set(Some(env.writes.get() + k));
        let first = runtime.block_on(importer(&env).run("id-000001"));
        env.fail_at.set(None);
        *env.answers.borrow_mut() = vec![json!({"dynamic": "import"}); 400].into();
        let second = runtime.block_on(importer(&env).run("id-000001"));
        let state = normalized(&env);
        if first.is_ok() || second.is_err() || !same(&state, &clean_py) {
            let keys: Vec<String> = state
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| !same(v, &clean_py[k.as_str()]))
                .map(|(k, v)| {
                    format!(
                        "{k}: rust {} / python {}",
                        short(v),
                        short(&clean_py[k.as_str()])
                    )
                })
                .collect();
            diffs.push(format!(
                "fail at write {k}: first {:?}, second {:?}\n  {}",
                first.err().map(|e| e.to_string()),
                second.err().map(|e| e.to_string()),
                keys.join("\n  ")
            ));
        }
    }
    eprintln!(
        "import restarts: {total} write boundaries in Rust ({} in Python, {python_lost} of whose restarts lost or \
         doubled work); {} differ from the clean run",
        imp["writes"],
        diffs.len()
    );
    for d in diffs.iter().take(6) {
        eprintln!("{d}\n");
    }
    assert!(diffs.is_empty());
}

/// decision: MEM-421. The day's distill failing at each of its writes, then run again on the next hourly pass:
/// the same outcome, events and tables as Python (no fix here; T-1044's retry rules are unchanged).
#[test]
fn distill_restart_at_every_write_matches_python() {
    let data = fixture("restarts.json");
    let d = &data["distill"];
    let tmp = tempfile::tempdir().unwrap();
    let runtime = rt();
    let setup = |name: &str| {
        let env = Env::new(name, Some(tmp.path()));
        for step in d["setup"].as_array().unwrap() {
            runtime
                .block_on(op(&env, step["op"].as_str().unwrap(), &step["args"]))
                .unwrap();
        }
        env
    };
    let clean_env = setup("dr-count");
    *clean_env.answers.borrow_mut() = vec![d["answer"].clone()].into();
    let before = clean_env.writes.get();
    runtime
        .block_on(clean_env.distiller().run_day("2026-10-19"))
        .unwrap();
    let total = clean_env.writes.get() - before;
    let clean = dump(&clean_env);
    let mut diffs = Vec::new();
    if total != d["writes"].as_u64().unwrap() {
        diffs.push(format!("writes: rust {total}, python {}", d["writes"]));
    }
    for (t, rows) in &clean {
        if !same(rows, &d["clean"][t]) {
            diffs.push(format!(
                "clean table {t}\n  rust:   {}\n  python: {}",
                short(rows),
                short(&d["clean"][t])
            ));
        }
    }
    for run in d["runs"].as_array().unwrap() {
        let k = run["fail_at"].as_u64().unwrap();
        let env = setup(&format!("dr-{k}"));
        *env.answers.borrow_mut() = vec![d["answer"].clone(), d["answer"].clone()].into();
        env.fail_at.set(Some(env.writes.get() + k));
        for (n, want) in run["attempts"].as_array().unwrap().iter().enumerate() {
            env.events.lock().unwrap().clear();
            let got = outcome(runtime.block_on(env.distiller().run_day("2026-10-19")));
            env.fail_at.set(None);
            let events = Value::Array(env.events.lock().unwrap().clone());
            if !same(&got, &want["out"]) || !same(&events, &want["events"]) {
                diffs.push(format!(
                    "fail at {k}, attempt {n}\n  rust:   {}\n  python: {}",
                    short(&got),
                    short(&want["out"])
                ));
            }
        }
        let tables = dump(&env);
        let want = run["tables"].as_object().unwrap();
        for (t, rows) in &tables {
            let expected = want.get(t).unwrap_or(&d["clean"][t]);
            if !same(rows, expected) {
                diffs.push(format!(
                    "fail at {k}: table {t}\n  rust:   {}\n  python: {}",
                    short(rows),
                    short(expected)
                ));
            }
        }
    }
    report(
        "distill restarts",
        d["runs"].as_array().unwrap().len(),
        &diffs,
    );
    assert!(diffs.is_empty());
}

// decision: MEM-419, MEM-420, MEM-30, MEM-31
#[test]
fn pure_rules() {
    let data = fixture("pure.json");
    let mut diffs = Vec::new();
    let mut n = 0;
    for c in data["text"].as_array().unwrap() {
        n += 1;
        let (t, p) = (c["text"].as_str().unwrap(), c["part"].as_str().unwrap());
        let span = |o: Option<(usize, usize)>| o.map_or(Value::Null, |(a, b)| json!([a, b]));
        let got = json!({"text": t, "part": p, "contains": text::contains_words(t, p), "found": span(text::found(t, p)),
                         "clauses": text::owner_clauses(t, p), "whole": text::whole_clauses(t, p),
                         "quoted": text::quoted(t, p, false), "quoted_clause": text::quoted(t, p, true)});
        if got != *c {
            diffs.push(format!("text\n  rust:   {got}\n  python: {c}"));
        }
    }
    for c in data["capture"].as_array().unwrap() {
        n += 1;
        let got = text::capture(
            c["own"].as_str().unwrap(),
            c["content"].as_str().unwrap(),
            c["quote"].as_str().unwrap(),
        );
        let got = json!({"own": c["own"], "content": got.content, "quote": c["quote"], "origin": got.origin, "refused": got.refused});
        if got != *c {
            diffs.push(format!("capture\n  rust:   {got}\n  python: {c}"));
        }
    }
    for c in data["fingerprint"].as_array().unwrap() {
        n += 1;
        let a: Vec<&str> = c["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        if text::fingerprint(a[0], a[1], a[2], a[3]) != c["out"] {
            diffs.push(format!("fingerprint {c}"));
        }
    }
    for c in data["lineage"].as_array().unwrap() {
        n += 1;
        if ferb_learning::lineage::direct(c["ref"].as_str().unwrap()) != c["direct"] {
            diffs.push(format!("direct {c}"));
        }
    }
    for c in data["lineage_calls"].as_array().unwrap() {
        n += 1;
        let got: Vec<String> =
            ferb_learning::lineage::call_sources(&c["input"], Some(&c["result"]))
                .into_iter()
                .collect();
        if json!(got) != c["out"] {
            diffs.push(format!("call_sources {c} → {got:?}"));
        }
    }
    for c in data["notes"].as_array().unwrap() {
        n += 1;
        if let Some(pairs) = c.get("slug_words") {
            for pair in pairs.as_array().unwrap() {
                if ferb_learning::notes::slug_words(pair[0].as_str().unwrap()) != pair[1] {
                    diffs.push(format!("slug_words {pair}"));
                }
            }
            continue;
        }
        let (kind, key, body) = (
            c["kind"].as_str().unwrap(),
            c["key"].as_str().unwrap(),
            &c["body"],
        );
        let got = json!({"kind": kind, "key": key, "body": body,
            "search_text": ferb_learning::notes::search_text(kind, key, body),
            "shown_text": ferb_learning::notes::shown_text(kind, key, body, "2026-10-20T09:00:00+00:00"),
            "how_note": ferb_learning::notes::how_note(body),
            "shown_state": ferb_learning::notes::shown_state(kind, body, "active"),
            "encoded": ferb_learning::notes::encoded(body),
            "digest": ferb_learning::notes::digest(&json!([kind, key, body]))});
        if got != *c {
            diffs.push(format!("notes\n  rust:   {got}\n  python: {c}"));
        }
    }
    for c in data["origin"].as_array().unwrap() {
        n += 1;
        if ferb_learning::notes::memory_origin(c[0].as_str().unwrap()) != c[1] {
            diffs.push(format!("memory_origin {c}"));
        }
    }
    for c in data["journal_origin"].as_array().unwrap() {
        n += 1;
        if ferb_learning::journal::journal_origin(c[0].as_str().unwrap(), &c[1]) != c[2] {
            diffs.push(format!("journal_origin {c}"));
        }
    }
    for c in data["attempts"].as_array().unwrap() {
        n += 1;
        if json!(ferb_learning::playbooks::attempts(&c[0])) != c[1] {
            diffs.push(format!("attempts {c}"));
        }
    }
    for c in data["group_key"].as_array().unwrap() {
        n += 1;
        let ids: Vec<String> = c[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect();
        if ferb_learning::playbooks::group_key(&ids) != c[1] {
            diffs.push(format!("group_key {c}"));
        }
    }
    report("pure", n, &diffs);
    assert!(diffs.is_empty());
}
