//! rust/fixtures/daemon/rpc_transcripts.json, recorded from the real Python daemon, replayed end to end through the
//! real `ferbd daemon` program: the same lines ferb-daemon's own tests replay in-process
//! (`crates/ferb-daemon/tests/a_replay.rs`, `b_small.rs`), with the same stand-ins, put in through the test build's
//! `FERBD_STAND_INS`. Each answer line must come back byte for byte, or as the named fix (ENG-6) says.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use support::*;

/// a_replay.rs's FIXED: (scenario, request id) → the refusal Rust answers where Python failed inside.
const FIXED: &[(&str, i64, &str)] = &[
    (
        "memory",
        4,
        "memory.add needs content: the words to remember",
    ),
    ("memory", 8, "memory.show needs a subject"),
    (
        "memory",
        11,
        "memory.edit needs the memory's id and its new content",
    ),
    ("memory", 13, "memory.retract needs the memory's id"),
    ("memory", 18, "memory.forget needs the memory's id"),
    (
        "schedules_reminders_timers",
        8,
        "schedules.change needs a schedule's id and an action: cancel, pause or resume",
    ),
    (
        "schedules_reminders_timers",
        15,
        "reminders.cancel needs a reminder's id",
    ),
    (
        "settings_setup_about",
        76,
        "since must be a date and time, like 2026-01-01T09:00:00+00:00",
    ),
];

const GROUP_A: [&str; 26] = [
    "memory.learning",
    "memory.list",
    "memory.show",
    "memory.add",
    "memory.profile",
    "memory.edit",
    "memory.retract",
    "memory.forget",
    "memory.restore",
    "memory.repeats",
    "timeline.get",
    "setup.persona",
    "memory.import.estimate",
    "memory.import.start",
    "memory.import.status",
    "memory.import.cancel",
    "schedules.list",
    "schedules.change",
    "reminders.list",
    "reminders.cancel",
    "timers.repeat",
    "timers.states",
    "chat.history.put",
    "chat.history.import",
    "chat.history.page",
    "chat.history.days",
];

fn ours(method: &str) -> bool {
    method == "auth.hello"
        || method == "settings.get"
        || method == "settings.set"
        || GROUP_A.contains(&method)
}

/// Walks a scenario's steps for the connections `only` names (all when empty), sending group A's requests.
fn replay_a(name: &str, run: &Run, root: &Path, only: &[&str]) -> (usize, usize) {
    replay_a_with(name, run, root, only, None)
}

/// `before`: a control request made just before the request with that id is sent.
fn replay_a_with(
    name: &str,
    run: &Run,
    root: &Path,
    only: &[&str],
    before: Option<(i64, Value)>,
) -> (usize, usize) {
    let scenario = scenario(&fixture("daemon/rpc_transcripts.json"), name);
    let fixed: BTreeMap<i64, &str> = FIXED
        .iter()
        .filter(|(s, _, _)| *s == name)
        .map(|(_, id, m)| (*id, *m))
        .collect();
    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    let (mut same, mut named) = (0, 0);
    for step in scenario["steps"].as_array().unwrap() {
        let conn = step["conn"].as_str().unwrap_or("").to_owned();
        if !only.is_empty() && !conn.is_empty() && !only.contains(&conn.as_str()) {
            continue;
        }
        match step["do"].as_str().unwrap() {
            "connect" => {
                conns.insert(conn, Raw::connect(&run.socket()).unwrap());
            }
            "close" => {
                conns.remove(&conn);
            }
            "clock" => {
                run.control(json!({"do": "clock", "advance_s": step["advance_s"]}));
            }
            "timers_tick" => {
                let fired = run.control(json!({"do": "timers_tick"}));
                assert_eq!(fired, step["fired"], "{name}: timers fired");
            }
            "call" => {
                let lines = step["lines"].as_array().unwrap();
                let sent = line_text(&lines[0]["send"]);
                let (method, id) = method_of(&sent);
                if !ours(&method) {
                    continue;
                }
                if let Some((at, req)) = &before {
                    if id == Some(*at) {
                        run.control(req.clone());
                    }
                }
                let c = conns.get_mut(&conn).unwrap();
                c.send(&sent);
                for want in lines[1..]
                    .iter()
                    .filter_map(|l| l.get("recv"))
                    .map(line_text)
                {
                    let got = fx(&c.recv().expect("an answer"), root);
                    match id.and_then(|i| fixed.get(&i)) {
                        Some(message) => {
                            let python: Value = serde_json::from_str(&want).unwrap();
                            assert_eq!(
                                python["error"]["code"], -32603,
                                "{name} {id:?}: Python's answer"
                            );
                            let rust = ferb_rpc::wire::error_line(
                                &ferb_rpc::Id::Int(serde_json::Number::from(id.unwrap())),
                                -32004,
                                message,
                            );
                            assert_eq!(got.as_bytes(), &rust[..rust.len() - 1], "{name} {id:?}");
                            run.control(json!({"do": "ids_bump"})); // the id Python's internal-error row took
                            named += 1;
                        }
                        None => {
                            assert_eq!(got, want, "{name}: {method} (id {id:?})");
                            same += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (same, named)
}

/// The end tables group A writes, against the recording's (journal rows from `journal_from` on).
fn tables(name: &str, run: &Run, names: &[&str], journal_from: &str) {
    let scenario = scenario(&fixture("daemon/rpc_transcripts.json"), name);
    for t in names {
        let want = scenario["tables"][*t]["rows"].clone();
        let got = run.table(t);
        if *t == "journal" {
            let pick = |rows: &Value| -> Vec<Value> {
                rows.as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| {
                        r[1].as_str().unwrap_or("") >= journal_from
                            && r[3] != "daemon.internal_error"
                    })
                    .map(|r| json!(r.as_array().unwrap()[1..]))
                    .collect()
            };
            assert_eq!(json!(pick(&got)), json!(pick(&want)), "{name}: journal");
            continue;
        }
        assert_eq!(
            got,
            if want.is_null() { json!([]) } else { want },
            "{name}: table {t}"
        );
    }
}

fn base() -> Value {
    // the recorder's programs (its start wrote 14 rows, the replay's ids continue at 15)
    json!({"ids": "id", "group_a": true, "programs": "recorder"})
}

#[test]
fn memory_through_the_binary() {
    let tmp = root();
    let run = Run::start(tmp.path(), "05", base());
    run.control(json!({"do": "ids_next", "n": 15}));
    let (same, named) = replay_a("memory", &run, tmp.path(), &[]);
    assert_eq!((same, named), (20, 5));
    tables(
        "memory",
        &run,
        &[
            "memory_items",
            "memory_sources",
            "memory_forgotten",
            "journal",
        ],
        "id-000015",
    );
}

#[test]
fn chat_history_through_the_binary() {
    let tmp = root();
    let run = Run::start(tmp.path(), "06", base());
    let (same, named) = replay_a("chat_history", &run, tmp.path(), &[]);
    assert_eq!((same, named), (12, 0));
    tables("chat_history", &run, &["chat_history"], "");
}

#[test]
fn protocol_app_only_refusal_through_the_binary() {
    let tmp = root();
    let mut cfg = base();
    cfg["app_check"] = json!(false);
    let run = Run::start(tmp.path(), "01", cfg);
    let (same, named) = replay_a("protocol", &run, tmp.path(), &["not-the-app"]);
    assert_eq!((same, named), (2, 0));
}

#[test]
fn schedules_through_the_binary() {
    let tmp = root();
    let run = Run::start(tmp.path(), "09", base());
    let at = "2026-01-01T09:00:00+00:00";
    let timer = |id: &str, kind: &str, fire: &str, msg: &str, req: Value| {
        json!(["INSERT INTO timers (id, kind, fire_at, message, task_id, request_id, state, created_at) VALUES \
                (?, ?, ?, ?, NULL, ?, 'pending', ?)", [id, kind, fire, msg, req, at]])
    };
    let statements = json!([
        timer("tm-id-000022", "reminder", "2026-01-01T09:30:00+00:00", "Call the invented plumber", json!("id-000015")),
        ["INSERT INTO timer_names VALUES ('tm-id-000022', 'Plumber')", []],
        timer("tm-id-000024", "timer", "2026-01-01T09:10:00+00:00", "Tea is ready", json!("id-000015")),
        ["INSERT INTO timer_names VALUES ('tm-id-000024', 'Tea')", []],
        timer("tm-id-000028", "schedule", "2026-01-02T08:00:00+00:00", "sc-id-000027", Value::Null),
        ["INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, timer_id, state, \
          created_at, watch) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, 'none')",
         ["sc-id-000027", "Water the invented plants",
          "water the plants every morning at 8, remind me in 30 minutes to call the plumber, and set a 10 minute timer for tea",
          "{\"apps\": [], \"sites\": [], \"paths\": [], \"recipients\": [], \"run_scope\": {\"folders\": [], \"identity\": \"owner\", \"worker_type\": \"default\", \"profile\": \"\", \"account\": \"\"}}",
          "{\"repeat\": \"daily\", \"at\": \"08:00\", \"weekday\": null, \"every_minutes\": null, \"tz\": \"UTC\", \"when_active\": false, \"first\": \"2026-01-02T08:00:00+00:00\"}",
          "2026-01-02T08:00:00+00:00", "tm-id-000028", at]],
        ["INSERT INTO schedule_names VALUES ('sc-id-000027', 'Plant watering')", []],
    ]);
    run.control(json!({"do": "sql", "statements": statements}));
    run.control(json!({"do": "ids_next", "n": 32}));
    run.control(json!({"do": "tokens", "n": 3}));
    let (same, named) = replay_a("schedules_reminders_timers", &run, tmp.path(), &["app#2"]);
    assert_eq!((same, named), (20, 2));
    tables(
        "schedules_reminders_timers",
        &run,
        &[
            "timers",
            "timer_names",
            "timer_repeats",
            "timer_deliveries",
            "schedule_names",
            "journal",
        ],
        "id-000032",
    );
    let want = scenario(
        &fixture("daemon/rpc_transcripts.json"),
        "schedules_reminders_timers",
    )["tables"]["schedules"]["rows"]
        .clone();
    assert_eq!(run.table("schedules"), want);
    // the seams: the timer's banner (group A's stand-in records it)
    let calls = run.control(json!({"do": "calls"}));
    let notices: Vec<&Value> = calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("notify").is_some())
        .collect();
    assert_eq!(notices.len(), 1, "{calls}");
    assert_eq!(notices[0]["notify"]["title"], "Tea");
}

#[test]
fn settings_import_timeline_persona_through_the_binary() {
    let tmp = root();
    let (_, user) = Run::paths(tmp.path(), "30");
    let project = user.join(".claude/projects/invented-project");
    std::fs::create_dir_all(&project).unwrap();
    let rows = [
        json!({"type": "user", "timestamp": "2025-12-30T10:00:00Z", "message": {"role": "user", "content": "I keep my invented notes in ~/notes"}}),
        json!({"type": "assistant", "timestamp": "2025-12-30T10:00:05Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
    ];
    let text: String = rows
        .iter()
        .map(|r| ferb_py::json::dumps(r) + "\n")
        .collect();
    std::fs::write(project.join("session-1.jsonl"), text).unwrap();
    let run = Run::start(tmp.path(), "30", base());
    // the journal's newest rows as the scenario left them before `timeline.get` (the response shows them)
    let recorded = scenario(
        &fixture("daemon/rpc_transcripts.json"),
        "settings_setup_about",
    )["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["lines"].as_array().cloned())
        .flatten()
        .filter_map(|l| l.get("recv").map(line_text))
        .find(|l| l.contains("\"id\": 75,"))
        .map(|l| serde_json::from_str::<Value>(&l).unwrap())
        .unwrap();
    // the start's own rows go first: the timeline reads the newest, as the recording's start had none of them
    let mut seed = vec![json!(["DELETE FROM journal", []]), json!([
        "INSERT INTO journal (id, at, kind, data_json, origin) VALUES ('seed-0', ?, 'daemon.started', '{}', 'ferb')",
        ["2026-01-01T09:00:00+00:00"]])];
    for (n, e) in recorded["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        seed.push(json!([
            "INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, 'ferb')",
            [
                format!("seed-{}", n + 1),
                e["at"],
                e["kind"],
                ferb_py::json::dumps(&e["data"])
            ]
        ]));
    }
    // seeded just before `timeline.get` (id 75): the journal as the recording had it then
    let (same, named) = replay_a_with(
        "settings_setup_about",
        &run,
        tmp.path(),
        &[],
        Some((75, json!({"do": "sql", "statements": seed}))),
    );
    assert_eq!((same, named), (15, 1));
}

// ---------------------------------------------------------------------------------------------------------------
// group B's small areas (b_small.rs `base_recordings_of_settings_setup_about`)
// ---------------------------------------------------------------------------------------------------------------

const MINE: &[&str] = &[
    "about.guide",
    "secrets.saved",
    "secrets.delete",
    "seen.list",
    "seen.image",
    "sense.pause",
    "sense.resume",
    "sense.status",
    "jev.stats",
    "jev.enable",
    "jev.reload",
    "notifications.categories",
    "attention.ack",
    "notices.list",
    "intro.suggest",
];

/// b_small.rs `fixes`: the internal errors Python gave bad parameters; Rust refuses in the area's own words.
fn b_fix(request: &str, python: &str) -> Option<String> {
    let v: Value = serde_json::from_str(request).ok()?;
    if !python.contains("-32603") {
        return None;
    }
    let words = match (v["method"].as_str()?, &v["params"]) {
        ("attention.ack", p) if p.get("ids") == Some(&json!(["no-such-notice"])) => {
            "No such owner notice"
        }
        ("attention.ack", _) => "ids must be a nonempty list of notice ids",
        ("jev.enable", _) => "name the question to switch on or off",
        ("seen.list", _) => "limit must be a number of screenshots",
        _ => return None,
    };
    Some(ferb_py::json::dumps(
        &json!({"jsonrpc": "2.0", "id": v["id"], "error": {"code": -32004, "message": words}}),
    ))
}

#[test]
fn group_b_small_areas_through_the_binary() {
    let file = fixture("daemon/rpc_transcripts.json");
    let s = scenario(&file, "settings_setup_about");
    let steps = s["steps"].as_array().unwrap();
    let perception = steps
        .iter()
        .flat_map(|st| st["lines"].as_array().cloned().unwrap_or_default())
        .filter_map(|l| l.get("recv").map(line_text))
        .find(|l| l.contains("\"perception\""))
        .map(|l| serde_json::from_str::<Value>(&l).unwrap()["result"]["perception"].clone())
        .unwrap();
    let tmp = root();
    let run = Run::start(
        tmp.path(),
        "40",
        json!({"ids": "id", "group_a": true, "group_b": "small", "perception": perception}),
    );
    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    let (mut same, mut named, mut diffs) =
        (BTreeMap::<String, usize>::new(), 0, Vec::<String>::new());
    for st in steps {
        let label = st["conn"].as_str().unwrap_or("").to_owned();
        match st["do"].as_str().unwrap_or("") {
            "connect" => {
                conns.insert(label, Raw::connect(&run.socket()).unwrap());
            }
            "close" => {
                conns.remove(&label);
            }
            "call" => {
                let lines = st["lines"].as_array().unwrap();
                let request = line_text(&lines[0]["send"]);
                let (method, _) = method_of(&request);
                // about.get reads the settings the scenario's settings.set calls changed: those are sent too
                let with = ["about.get", "settings.get", "settings.set"];
                if method != "auth.hello"
                    && !MINE.contains(&method.as_str())
                    && !with.contains(&method.as_str())
                {
                    continue;
                }
                let c = conns.get_mut(&label).unwrap();
                c.send(&request);
                for l in &lines[1..] {
                    if let Some(r) = l.get("recv") {
                        let want = line_text(r);
                        let (want, is_fix) = match b_fix(&request, &want) {
                            Some(f) => (f, true),
                            None => (want, false),
                        };
                        // the database's size is the recorder's own store's
                        let got = c
                            .recv()
                            .map(|g| scrub(&fx(&g, tmp.path()), &["database_bytes"]));
                        let want = scrub(&want, &["database_bytes"]);
                        if got.as_deref() == Some(want.as_str()) {
                            if is_fix {
                                named += 1;
                            } else {
                                *same.entry(method.clone()).or_default() += 1;
                            }
                        } else {
                            diffs.push(format!("{method}:\n  python: {want}\n  rust:   {got:?}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    eprintln!("same {same:?} named {named}");
    assert_eq!(named, 2, "attention.ack's two named fixes");
    assert_eq!(same.get("about.get"), Some(&1), "about.get compared");
    for m in MINE {
        if *m != "seen.image" {
            assert!(
                same.contains_key(*m) || *m == "attention.ack",
                "{m} not compared"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// group B's runtimes, browsers and shell processes on the real composition (Keychain off, no Full Disk Access)
// ---------------------------------------------------------------------------------------------------------------

/// b_support's masks: these keys' values are `"<volatile>"` at any depth.
fn scrub(line: &str, keys: &[&str]) -> String {
    if keys.is_empty() {
        return line.to_owned();
    }
    let Ok(mut v) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    fn walk(v: &mut Value, keys: &[&str]) {
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if keys.contains(&k.as_str()) {
                        *x = Value::String("<volatile>".into());
                    } else {
                        walk(x, keys);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, keys)),
            _ => {}
        }
    }
    walk(&mut v, keys);
    ferb_py::json::dumps(&v)
}

/// The recording's `/FX/<nn>/home` as this run's home.
fn localize(text: &str, home: &Path) -> String {
    let re = regex_lite(text);
    re.replace("{HOME}", &home.to_string_lossy())
}

fn regex_lite(text: &str) -> String {
    // `/FX/<two digits>/home` → `{HOME}`
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("/FX/") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 4..];
        let b = tail.as_bytes();
        if b.len() >= 7 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && &tail[2..7] == "/home"
        {
            out.push_str("{HOME}");
            rest = &tail[7..];
        } else {
            out.push_str("/FX/");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// b_support's `replay`: connections, calls and streams of the methods `keep` accepts, each answer compared.
fn replay_b(
    run: &Run,
    steps: &[Value],
    keep: &dyn Fn(&str) -> bool,
    volatile: &[&str],
) -> (BTreeMap<String, usize>, Vec<String>) {
    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    let mut streams: BTreeMap<String, String> = BTreeMap::new();
    let (mut same, mut diffs) = (BTreeMap::<String, usize>::new(), Vec::new());
    let mut check = |c: &mut Raw, method: &str, want: String| {
        let want = scrub(&localize(&want, &run.home), volatile);
        let got = c.recv().map(|g| scrub(&g, volatile));
        if got.as_deref() == Some(want.as_str()) {
            *same.entry(method.to_string()).or_default() += 1;
        } else {
            diffs.push(format!("{method}:\n  python: {want}\n  rust:   {got:?}"));
        }
    };
    for st in steps {
        let label = st["conn"].as_str().unwrap_or("").to_owned();
        match st["do"].as_str().unwrap_or("") {
            "connect" => {
                conns.insert(label, Raw::connect(&run.socket()).unwrap());
            }
            "close" => {
                conns.remove(&label);
            }
            "call" => {
                let lines = st["lines"].as_array().unwrap();
                let Some(first) = lines.first().and_then(|l| l.get("send")) else {
                    continue;
                };
                let request = localize(&line_text(first), &run.home);
                let (method, _) = method_of(&request);
                if method != "auth.hello" && !keep(&method) {
                    continue;
                }
                let c = conns.get_mut(&label).unwrap();
                c.send(&request);
                for l in &lines[1..] {
                    if let Some(r) = l.get("recv") {
                        check(c, &method, line_text(r));
                    }
                }
            }
            "stream" => {
                let request = localize(&line_text(&st["lines"][0]["send"]), &run.home);
                let (method, _) = method_of(&request);
                if !keep(&method) {
                    continue;
                }
                conns.get_mut(&label).unwrap().send(&request);
                streams.insert(label, method);
            }
            "stream_end" | "stream_cut" => {
                let Some(method) = streams.remove(&label) else {
                    continue;
                };
                let Some(c) = conns.get_mut(&label) else {
                    continue;
                };
                for l in st["lines"].as_array().unwrap() {
                    if let Some(r) = l.get("recv") {
                        check(c, &method, line_text(r));
                    }
                }
            }
            _ => {}
        }
    }
    (same, diffs)
}

#[test]
fn runtimes_signed_out_through_the_binary() {
    let file = fixture("daemon/rpc_transcripts.json");
    let tmp = root();
    let run = Run::start(
        tmp.path(),
        "50",
        json!({"group_a": true, "programs": "recorder"}),
    );
    let mut total = 0;
    for (name, prefix) in [("runtimes", "runtimes."), ("settings_setup_about", "keys.")] {
        let sc = scenario(&file, name);
        let (same, diffs) = replay_b(
            &run,
            sc["steps"].as_array().unwrap(),
            &|m| m.starts_with(prefix),
            &["pid"],
        );
        assert!(
            diffs.is_empty(),
            "{name}: {} difference(s):\n{}",
            diffs.len(),
            diffs.join("\n")
        );
        total += same
            .iter()
            .filter(|(m, _)| m.starts_with(prefix))
            .map(|x| x.1)
            .sum::<usize>();
    }
    assert!(total > 0);
    eprintln!("runtimes and keys: {total} lines");
    let _ = &run;
}

#[test]
fn browsers_without_full_disk_access_through_the_binary() {
    const BROWSERS: [&str; 13] = [
        "browsers.list",
        "browsers.pair",
        "browsers.installed",
        "browsers.unpair",
        "setup.browser_pair",
        "setup.browsers_pair_all",
        "setup.browsers_pairing",
        "setup.browsers",
        "setup.browser_open",
        "browser.show_handoff",
        "identity.find",
        "identity.choose",
        "identity.pair",
    ];
    let file = fixture("daemon/rpc_transcripts.json");
    let tmp = root();
    let run = Run::start(tmp.path(), "51", json!({"group_a": true}));
    let sc = scenario(&file, "browsers");
    let (same, diffs) = replay_b(
        &run,
        sc["steps"].as_array().unwrap(),
        &|m| BROWSERS.contains(&m),
        &[],
    );
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    assert_eq!(same.values().sum::<usize>(), 32, "{same:?}"); // 31 calls' lines and the hello
}

#[test]
fn shell_processes_through_the_binary() {
    let file = fixture("daemon/rpc_transcripts.json");
    let tmp = root();
    let run = Run::start(tmp.path(), "52", json!({"group_a": true}));
    let sc = scenario(&file, "streams_and_processes");
    let keep = |m: &str| m.starts_with("ps.") || m == "notifications.subscribe";
    let (same, diffs) = replay_b(&run, sc["steps"].as_array().unwrap(), &keep, &[]);
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    eprintln!("{same:?}");
    for m in ["ps.list", "ps.kill", "ps.send", "ps.attach"] {
        assert!(same.contains_key(m), "{m} not compared: {same:?}");
    }
}
