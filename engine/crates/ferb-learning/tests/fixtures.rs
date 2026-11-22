//! Replays `engine/fixtures/learning` (made by running the Python code, ENG-3) through the Rust port and reports every
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
    replay_on(&env, sc, root, &runtime)
}

fn replay_on(
    env: &Env,
    sc: &Value,
    root: &std::path::Path,
    runtime: &tokio::runtime::Runtime,
) -> (usize, Vec<String>) {
    let name = sc["name"].as_str().unwrap();
    let mut diffs = Vec::new();
    let mut before = hide_root(Value::Object(dump(env)), root)
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
        let got = hide_root(outcome(runtime.block_on(op(env, op_name, &args))), root);
        let events = hide_root(Value::Array(env.events.lock().unwrap().clone()), root);
        let after = hide_root(Value::Object(dump(env)), root)
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

/// decision: MEM-400, MEM-402, MEM-410, MEM-411, MEM-412; MEM-439 (named difference, the owner's IMP-20 and IMP-31):
/// Python saved each chunk's checked facts as memory items at once; FERB keeps them as the import's checked facts and
/// places them into the profile after the run (`place`), so the items, their sources and every id after them differ.
/// The reading is Python's: each scenario is replayed strictly up to its first run, and that run sends every model
/// call Python sent (the same system prompt and text, in order), marks the same parts done, ends the same way, and
/// keeps every fact Python saved among its checked facts.
/// MEM-454 (named difference, the owner's yes to the memory-import lab's measured prompts): recall and verify read
/// the new import_recall.md and import_verify.md, and verify names a reading where Python's said `supported` and
/// `kind`. Python's recorded verify answers are given as readings ([`as_reading`]), and each call made with a new
/// prompt is compared as made with the one Python read ([`as_python_call`]); every other byte is compared as is.
#[test]
fn memory_import_runs() {
    let data = fixture("scenarios.json");
    let tmp = tempfile::tempdir().unwrap();
    let runtime = rt();
    let mut diffs = Vec::new();
    let mut runs = 0;
    for sc in data["imports"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let root = tmp.path();
        let folder = root.join(name).join("export");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("conversations.json"), sc["export"].to_string()).unwrap();
        let env = Env::new(name, Some(root));
        let mut cut = sc.clone();
        let steps = sc["steps"].as_array().unwrap();
        let first_run = steps.iter().position(|s| s["op"] == "import.run").unwrap();
        cut["steps"] = Value::Array(steps[..first_run].to_vec());
        let (_, before) = replay_on(&env, &cut, root, &runtime);
        diffs.extend(before);
        let step = &steps[first_run];
        let answers: Vec<Value> = step["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e[0] == "model.start")
            .map(|e| as_reading(&e[1]["answer"]))
            .collect();
        *env.answers.borrow_mut() = answers.into();
        env.events.lock().unwrap().clear();
        let args: Value = serde_json::from_str(
            &step["args"]
                .to_string()
                .replace("/FX", &root.to_string_lossy()),
        )
        .unwrap();
        let got = runtime.block_on(op(&env, "import.run", &args)).unwrap();
        runs += 1;
        // three chunks at once (IMP-21): the same calls, in the order the waits let them go
        let calls = |evs: &[Value]| -> Vec<String> {
            let mut v: Vec<String> = evs
                .iter()
                .filter(|e| e[0] == "model.start")
                .map(|e| as_python_call(&e[1]))
                .map(|e| json!([e["system_sha"], e["parts"], e["goal"], e["role"]]).to_string())
                .collect();
            v.sort();
            v
        };
        let mine = hide_root(Value::Array(env.events.lock().unwrap().clone()), root);
        // recall reads exactly what Python's did; verify is shown the held facts that look alike, and Python's run
        // held the facts its earlier chunks had saved, so only the number of verify calls compares
        let (ours, theirs) = (
            calls(mine.as_array().unwrap()),
            calls(step["events"].as_array().unwrap()),
        );
        let recall = |v: &[String]| -> Vec<String> {
            v.iter()
                .filter(|c| !c.contains("What FERB already remembers"))
                .cloned()
                .collect()
        };
        if recall(&ours) != recall(&theirs) || ours.len() != theirs.len() {
            let first = recall(&ours)
                .into_iter()
                .zip(recall(&theirs))
                .find(|(a, b)| a != b);
            diffs.push(format!("{name}: the run's model calls differ from Python's ({} rust, {} python): {first:?}", ours.len(), theirs.len()));
        }
        if got["status"] != step["out"]["ok"]["status"] {
            diffs.push(format!(
                "{name}: ended {} where Python ended {}",
                got["status"], step["out"]["ok"]["status"]
            ));
        }
        let done = hide_root(
            Value::Array(
                env.store
                    .query("SELECT * FROM memory_import_done ORDER BY rowid", sql![])
                    .unwrap()
                    .iter()
                    .map(|r| Value::Array(r.json()))
                    .collect(),
            ),
            root,
        );
        if let Some(want) = step["tables"].get("memory_import_done") {
            if !same(&done, want) {
                diffs.push(format!(
                    "{name}: done marks\n  rust:   {}\n  python: {}",
                    short(&done),
                    short(want)
                ));
            }
        }
        let kept: BTreeSet<(String, String, String, String)> = env
            .store
            .query(
                "SELECT kind, subject, key, content FROM memory_import_facts",
                sql![],
            )
            .unwrap()
            .iter()
            .map(|r| (r.s("kind"), r.s("subject"), r.s("key"), r.s("content")))
            .collect();
        for row in step["tables"]
            .get("memory_items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let fact = (
                py::py_str(&row[2]),
                py::py_str(&row[3]),
                py::py_str(&row[4]),
                py::py_str(&row[5]),
            );
            if !kept.contains(&fact) {
                diffs.push(format!(
                    "{name}: Python saved {fact:?}, not among the checked facts"
                ));
            }
        }
    }
    report("imports (to the first run)", runs, &diffs);
    assert!(runs == 2 && diffs.is_empty());
}

/// The verify schema the import sends (ferb-memory's) is the one the catalog names (ferb-context's `VERIFY_SCHEMA`).
// decision: MEM-454
#[test]
fn the_import_sends_the_catalogs_verify_schema() {
    assert_eq!(
        ferb_memory::import_extract::verify_schema(),
        ferb_context::schemas::verify_schema()
    );
}

/// MEM-454: a recorded verify answer as the verify FERB asks now gives it: `supported` and `kind` become the reading
/// that keeps that kind, or "not_said" where Python's verifier didn't support the candidate.
fn as_reading(answer: &Value) -> Value {
    let mut answer = answer.clone();
    let verdicts = answer
        .get_mut("answer")
        .and_then(|a| a.get_mut("verdicts"))
        .and_then(Value::as_array_mut);
    for v in verdicts.into_iter().flatten() {
        let v = v.as_object_mut().unwrap();
        let kind = v.shift_remove("kind").unwrap();
        let reading = if v.shift_remove("supported") == Some(json!(true)) {
            reading_of(kind.as_str().unwrap())
        } else {
            "not_said"
        };
        v.insert("reading".into(), json!(reading));
    }
    answer
}

/// MEM-454: a call made with one of the prompts replaced whole since the recording, as made with the text Python
/// read (engine/fixtures/catalog/prompt_replacements.json): its system named by that text's hash, and the budget line
/// in its parts reserving that text's tokens; any other call as it is.
fn as_python_call(call: &Value) -> Value {
    let all: Value = serde_json::from_str(
        &std::fs::read_to_string(repo().join("engine/fixtures/catalog/prompt_replacements.json"))
            .unwrap(),
    )
    .unwrap();
    let prompts = ferb_context::PromptRoot::new(repo().join("engine/runtime")).unwrap();
    let mut call = call.clone();
    for r in all["replacements"].as_array().unwrap() {
        let new = prompts.load(r["prompt"].as_str().unwrap(), &[]).unwrap();
        let old = r["old"].as_str().unwrap();
        if call["system_sha"] != json!(py::sha256_hex(&new)) {
            continue;
        }
        call["system_sha"] = json!(py::sha256_hex(old));
        let reserved =
            |text: &str| format!("\"reserved_tokens\": {},", ferb_context::count_tokens(text));
        for part in call["parts"].as_array_mut().unwrap() {
            let moved = part
                .as_str()
                .unwrap()
                .replace(&reserved(&new), &reserved(old));
            *part = json!(moved);
        }
    }
    call
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
    // MEM-439: the run keeps checked facts; the profile's items come after it
    let facts = sorted_rows(
        q("SELECT kind, subject, key, content, said_at FROM memory_import_facts")
            .iter()
            .map(|r| Value::Array(r.json()))
            .collect(),
    );
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
            // MEM-439: chunks read at once (IMP-21) that a crash cut short are read again, so the calls and what they
            // proposed are counted again; the work kept (facts, done marks, chunks done) is not
            for k in ["model_calls", "candidates", "dropped", "verified"] {
                c.as_object_mut().unwrap().remove(k);
            }
            json!([r.s_at(0), c])
        })
        .collect();
    let said: Vec<Value> = Vec::new();
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

/// decision: MEM-410, MEM-439. Fixed against Python (ENG-6, `import_job.py:186-190`): a chunk's facts, its source row, its
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
    // MEM-439 (named difference): the clean run marks Python's parts done, ends as Python's did, and keeps every
    // fact Python saved among its checked facts; each restarted run must then end exactly as the clean Rust run
    let facts_of = |v: &Value, n: usize| -> BTreeSet<String> {
        v["facts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| Value::Array(r.as_array().unwrap()[..n].to_vec()).to_string())
            .collect()
    };
    if !same(&clean["done"], &clean_py["done"])
        || clean["runs"]
            .as_array()
            .map(|r| r.iter().map(|x| x[0].clone()).collect::<Vec<_>>())
            != clean_py["runs"]
                .as_array()
                .map(|r| r.iter().map(|x| x[0].clone()).collect::<Vec<_>>())
        || !facts_of(&clean_py, 4).is_subset(&facts_of(&clean, 4))
    {
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
        if first.is_ok() || second.is_err() || !same(&state, &clean) {
            let keys: Vec<String> = state
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| !same(v, &clean[k.as_str()]))
                .map(|(k, v)| {
                    format!(
                        "{k}: restarted {} / clean {}",
                        short(v),
                        short(&clean[k.as_str()])
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

// MEM-430 (fixed against Python): `quoted` also accepts a part whose words, in order, are the owner's when a pasted
// mark or line break sits between them; none of the 372 recorded text cases nor the 744 capture cases flips (none
// holds such a mark), so the comparison stays exact. The new cases are in capture_words.rs.
// MEM-431 (fixed against Python): a clause also ends where a period ends a sentence; no recorded case flips (their
// periods close "Dr." and "prof." before a capitalised name, or end the text). The new cases are in
// clause_sentences.rs.
// decision: MEM-419, MEM-420, MEM-30, MEM-31, MEM-430, MEM-431
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
