//! The front door against the planner on every recorded chat scenario (kb/plans/front-door.md, "Step 5 in detail",
//! Test): each scenario of tests/chat_replay.rs whose planner answers with today's answer form runs twice, side by side,
//! through the real `ferbd daemon` program with the test stand-ins' scripted model: once as chat_replay runs it (the
//! planner, the recorded answers; chat_replay shows this run is the Python recording's), and once with the front door on
//! (`front_door` in the script), each recorded planner answer converted mechanically into the front door's calls (each
//! part of the answer to its `chat.*` tool, the side entries first and the hand-off or question last, made through the
//! socket as the tool bridge makes them, with the run's token; a reply's text streamed as plain text). A repair (a
//! second planner call for the same message) becomes the same run's next calls, as the front door repairs inside its
//! run. A hand-off carries its ack and the task's goal (ORCH-743): the recorded planner call (and its repair, if any)
//! then writes the task, as the planner does after an accepted hand-off, reading its recorded live facts and role
//! instructions a second time. The effects are compared: the store's rows (tasks and their fields, plans, results,
//! timers, schedules, memory, the questions, cards answered, the conversation, the episodes), each table's ids numbered
//! afresh by first appearance, the journal's records, every later model call (the worker's and the checker's input:
//! what the hand-off started), the memory and browser ports' calls, the recorded tool calls' replies, every front
//! call's verdict, and the kinds of the frames each chat stream carried.
//!
//! The front door's turn writes records the planner's didn't (each call's span, ...), so the store hands out later ids
//! that differ from the recording's. In the front door's run the stand-in writes each recorded id that recorded
//! content names (a scripted tool call's arguments, a checker's evidence, a front call, a recorded read, a desktop
//! observer row) as the id this run made in its place, paired from what the run made (journal 265: the store's rows and
//! what the scripted model was shown, by order and kind; src/standins_chat.rs `Translation`), and the owner's recorded
//! lines are sent so too (`translate`), so every later effect is compared.
//!
//! Every difference is named in [`EXPECTED`] with its reason; a difference not named there fails the test, and so does
//! a named one that no longer happens. The scenarios run in [`BATCHES`] tests. `FRONT_REPLAY=<name>` runs one scenario;
//! `FRONT_REPLAY_DEBUG=1` prints the differences in full and each run's model calls and front calls;
//! `FRONT_REPLAY_IDS=1` lists each run's ids by record; `FRONT_REPLAY_KEEP=1` keeps both runs' folders.
//!
//! decision: ORCH-735, ORCH-736, ORCH-737, ORCH-743

mod support;
// the deliberate prompt changes since the recording (ENG-6), as chat_replay applies them to the recorded journal rows
#[path = "../../ferb-supervisor/tests/common/prompt_changes.rs"]
mod prompt_changes;
// APP-926's owner-facing words, applied to the newer recordings as chat_replay applies them
#[path = "../../ferb-supervisor/tests/common/named_diffs.rs"]
mod named_diffs;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use support::*;

// ---------------------------------------------------------------------------------------------------------------
// the recordings, read as chat_replay reads them
// ---------------------------------------------------------------------------------------------------------------

fn bodies() -> &'static HashMap<String, String> {
    static B: OnceLock<HashMap<String, String>> = OnceLock::new();
    B.get_or_init(|| {
        let mut out = HashMap::new();
        for rel in [
            "daemon/bodies.json",
            "supervisor/bodies.json",
            "supervisor/more_bodies.json",
            "supervisor/control_bodies.json",
        ] {
            if let Value::Object(m) = fixture(rel) {
                for (k, v) in m {
                    out.insert(k, prompt_changes::apply(v.as_str().unwrap()));
                }
            }
        }
        out
    })
}

fn resolve(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            json!(bodies()[m["sha256"].as_str().unwrap()])
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), resolve(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(resolve).collect()),
        other => other.clone(),
    }
}

fn s(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

/// A table cell's text: `@@body:<sha>:utf8@@` (quotes included) stands for the body as JSON.
fn expand(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(at) = out.find("\"@@body:") {
        let rest = &out[at + 8..];
        let end = rest.find("@@\"").expect("a body token's end");
        let (sha, form) = rest[..end].split_once(':').expect("a body token");
        let body = &bodies()[sha];
        let dumped = if form == "ascii" {
            ferb_py::json::dumps(&json!(body))
        } else {
            ferb_py::json::dumps_utf8(&json!(body))
        };
        out = format!("{}{}{}", &out[..at], dumped, &rest[end + 3..]);
    }
    out
}

fn cell(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("sha256") => {
            json!(prompt_changes::shift_with_system(&expand(
                &bodies()[m["sha256"].as_str().unwrap()]
            )))
        }
        Value::String(t) => json!(prompt_changes::shift_with_system(&expand(t))),
        other => other.clone(),
    }
}

fn line_of(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => s(&resolve(other)),
    }
}

/// How long the recorder's temporary root was (chat_replay's `recorded_root_len`).
fn recorded_root_len(chat: &Value) -> usize {
    let m = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
        .find(|m| m["role_played"] == "worker")
        .unwrap();
    let parts = strs(&resolve(&m["parts"]));
    let body = parts[..parts.len() - 1].join("\n\n");
    let report: Value =
        serde_json::from_str(parts[parts.len() - 1].trim_start_matches("Context budget: "))
            .unwrap();
    let extra = report["content_tokens_estimate"].as_u64().unwrap() as usize - body.len();
    3 + extra / body.matches("/FX").count()
}

/// The run's folder, as long as the recorder's (chat_replay's `RunRoot`).
struct RunRoot {
    root: String,
    resolved: String,
}

impl RunRoot {
    fn new(len: usize) -> (tempfile::TempDir, RunRoot) {
        let base = std::env::temp_dir();
        let base = base.to_str().unwrap().trim_end_matches('/').to_string();
        let name = len
            .checked_sub(base.len() + 1)
            .filter(|n| *n >= 3)
            .unwrap_or(6);
        let dir = tempfile::Builder::new()
            .prefix("fx")
            .rand_bytes(name - 2)
            .tempdir_in(&base)
            .unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        let resolved = std::fs::canonicalize(&root)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        (dir, RunRoot { root, resolved })
    }

    fn mask(&self, text: &str) -> String {
        text.replace(&self.resolved, "/FX")
            .replace(&self.root, "/FX")
    }
}

fn catalogs() -> &'static Value {
    static C: OnceLock<Value> = OnceLock::new();
    C.get_or_init(|| resolve(&fixture("supervisor/scenarios.json")["catalogs"]))
}

/// The newer recordings as chat_replay's `More` reads them (ORCH-642's topic and APP-926's words applied).
fn more() -> &'static Value {
    static M: OnceLock<Value> = OnceLock::new();
    M.get_or_init(|| {
        named_diffs::app_926(
            serde_json::from_str(
                &serde_json::to_string(&fixture("supervisor/more_scenarios.json"))
                    .unwrap()
                    .replace("browser.download_finished", "timer.fired"),
            )
            .unwrap(),
        )
    })
}

/// One recorded scenario with what its run needs: (scenario, its records, the catalogs, its folder number).
struct Recorded {
    sc: Value,
    recs: Vec<Value>,
    catalogs: Value,
    index: usize,
}

fn recorded() -> Vec<Recorded> {
    let mut out = Vec::new();
    let chat = fixture("daemon/chat_scenarios.json");
    let base = fixture("supervisor/scenarios.json");
    for (i, sc) in chat["scenarios"].as_array().unwrap().iter().enumerate() {
        let recs = base["scenarios"][s(&sc["name"])]["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(resolve)
            .collect();
        out.push(Recorded {
            sc: sc.clone(),
            recs,
            catalogs: catalogs().clone(),
            index: i,
        });
    }
    let m = more();
    let catalogs = resolve(&m["catalogs"]);
    for (i, sc) in m["scenarios"].as_array().unwrap().iter().enumerate() {
        let name = s(&sc["name"]);
        let recs = m["records"][&name]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let mut r = resolve(r);
                if let Some(key) = r["inputs"]["inputs"].as_str() {
                    r["inputs"] = resolve(&m["inputs"][key]);
                }
                r
            })
            .collect();
        let text = sc.to_string();
        let index = text
            .find("/FX/")
            .and_then(|at| text[at + 4..at + 6].parse::<usize>().ok())
            .map_or(60 + i, |n| n - 1);
        out.push(Recorded {
            sc: sc.clone(),
            recs,
            catalogs: catalogs.clone(),
            index,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// the conversion: a recorded planner answer as the front door's calls
// ---------------------------------------------------------------------------------------------------------------

/// A planner call of the recording: the chat turn's decision (not the attention check's, which shares its key).
fn planner_call(call: &Value) -> bool {
    call["key"] == "supervisor" && call["schema"] == "SUPERVISOR_SCHEMA"
}

/// The planner's answer in a recorded call: its form, when the call answered with one.
fn recorded_answer(call: &Value) -> Option<Value> {
    let fin = call["events"]
        .as_array()?
        .iter()
        .find(|e| e["kind"] == "final")?;
    let text = s(&fin["data"]["text"]);
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("kind").is_some().then_some(v)
}

/// The recorded answer as the front door's calls, in an order a model could make them (the control and the side
/// entries first, then the hand-off or the question, whose acceptance ends the run: a task part backed by an entry of
/// this message comes after that entry, ORCH-736), and its plain text ("" for a hand-off or a question, whose words are
/// the call's). `answers_question` is given (true) only when the answer says so and FERB's question is open.
fn convert(answer: &Value, question_open: bool) -> (Vec<(String, Value)>, String) {
    let side = &answer["side_entries"];
    let given = |v: &Value| match v {
        Value::Null | Value::Bool(false) => false,
        Value::Array(a) => !a.is_empty(),
        _ => true,
    };
    let aq = answer["answers_question"].as_bool().unwrap_or(false) && question_open;
    let mut calls = Vec::new();
    let mut part = |tool: &str, fields: &[(&str, &Value)]| {
        let mut args = Map::new();
        for (k, v) in fields {
            if given(v) {
                args.insert(k.to_string(), (*v).clone());
            }
        }
        if !args.is_empty() {
            if aq {
                args.insert("answers_question".into(), json!(true));
            }
            calls.push((tool.to_string(), Value::Object(args)));
        }
    };
    part(
        "chat.control_task",
        &[
            ("control", &answer["control"]),
            ("answer_question", &side["answer_question"]),
            ("conversation_changes", &side["conversation_changes"]),
        ],
    );
    part(
        "chat.set_reminder",
        &[
            ("timers", &side["timers"]),
            ("reminder_cancels", &side["reminder_cancels"]),
        ],
    );
    part(
        "chat.schedule",
        &[
            ("schedules", &side["schedules"]),
            ("schedule_changes", &side["schedule_changes"]),
            ("show_schedules", &side["show_schedules"]),
        ],
    );
    part(
        "chat.remember",
        &[("remember", &side["remember"]), ("forget", &side["forget"])],
    );
    let text = s(&answer["text"]);
    match answer["kind"].as_str() {
        // ORCH-743: the hand-off carries its ack and the task's goal; the planner then writes the task (the recorded
        // planner call after it, [`front_calls`])
        Some("task") => {
            let args = json!({"ack": text, "goal": answer["task"]["goal"], "answers_question": aq});
            calls.push(("chat.dispatch_task".into(), args));
            (calls, String::new())
        }
        Some("clarify") => {
            calls.push(("chat.ask".into(), json!({"question": text})));
            (calls, String::new())
        }
        _ => (calls, text),
    }
}

/// Why a recorded scenario isn't run through the front door, if it isn't: a planner call that never answered with the
/// form (FERB cancelled or left it, or the runtime failed), or no chat turn at all.
fn not_today_s_form(sc: &Value) -> Option<String> {
    let calls = sc["model_calls"].as_array().unwrap();
    let planner: Vec<&Value> = calls.iter().filter(|c| planner_call(c)).collect();
    let chat_sent =
        sc["steps"].as_array().unwrap().iter().any(|st| {
            st["do"] == "stream" && line_of(&st["lines"][0]["send"]).contains("chat.send")
        });
    if planner.is_empty() || !chat_sent {
        return Some("no chat turn (the typing preview and the attention check only)".into());
    }
    for (i, c) in planner.iter().enumerate() {
        if recorded_answer(&resolve(c)).is_none() {
            return Some(format!(
                "planner call {} has no answer form (FERB cancelled or left it, or the runtime failed)",
                i + 1
            ));
        }
    }
    None
}

/// The recorded calls with each planner call's events replaced by its converted calls. A repair (a second planner
/// call for the same message, after code refused the first answer) becomes part of the same run, as the front door
/// repairs inside its run: the first answer's calls (refused), then the second's; the repair's call is taken out.
/// ORCH-743: once the message's run hands off (an answer that was a task), the recorded planner call of that answer
/// follows the front door's, unchanged, and so does each recorded repair after it: the planner writes the task with
/// its own form, checks and repair, as it did in the recording (marked `handoff_planner`, so a recorded wait for the
/// nth planner call still means the nth front door call).
///
/// With the calls, how many times the front door's run reads each recorded planner call's reads (by its order among
/// the planner calls): the message's front door call reads its first call's, and each planner call writing the task
/// reads its own; a repair merged into the front door's run is read never.
fn front_calls(calls: &[Value]) -> (Vec<Value>, Vec<usize>) {
    let mut out: Vec<Value> = Vec::new();
    let mut reads: Vec<usize> = Vec::new();
    // whether FERB's question was open when the message came (a repair answers the same message)
    let (mut question_open, mut open_at_message) = (false, false);
    let mut last_request: Option<String> = None;
    // whether this message's front door run handed off: its later recorded calls are the planner's
    let mut handed = false;
    let planner_writes = |c: &Value| {
        let mut call = c.clone();
        call["handoff_planner"] = json!(true);
        call
    };
    for c in calls {
        if !planner_call(c) {
            out.push(c.clone());
            continue;
        }
        let answer = recorded_answer(c).expect("today's form");
        let request = s(&c["task_id"]);
        let repair = last_request.as_deref() == Some(request.as_str());
        if !repair {
            open_at_message = question_open;
            handed = false;
        }
        question_open = answer["kind"] == "clarify";
        last_request = Some(request);
        if handed {
            // the planner's repair of the task it writes for the hand-off
            out.push(planner_writes(c));
            reads.push(1);
            continue;
        }
        let (calls, text) = convert(&answer, open_at_message);
        let hands_off = calls.iter().any(|(name, _)| name == "chat.dispatch_task");
        let mut events: Vec<Value> = calls
            .iter()
            .map(|(name, args)| json!({"kind": "front_call", "name": name, "arguments": args}))
            .collect();
        if !text.is_empty() {
            events.push(json!({"kind": "text_delta", "data": {"text": text}}));
        }
        if let Some(usage) = c["events"]
            .as_array()
            .and_then(|e| e.iter().find(|e| e["kind"] == "usage"))
        {
            events.push(usage.clone());
        }
        events.push(json!({"kind": "final", "data": {"text": text}}));
        if repair {
            let prev = out
                .iter_mut()
                .rev()
                .find(|p| planner_call(p))
                .expect("the repaired call");
            let mut all = prev["events"].as_array().cloned().unwrap_or_default();
            // the first answer's own end goes: the model went on after its refused calls
            all.retain(|e| e["kind"] == "front_call");
            all.extend(events);
            prev["events"] = json!(all);
            reads.push(0);
        } else {
            let mut call = c.clone();
            call["events"] = json!(events);
            out.push(call);
            reads.push(1);
        }
        if hands_off {
            // ORCH-743: the accepted hand-off ends the run; the planner writes its task, as this call answered
            handed = true;
            out.push(planner_writes(c));
            *reads.last_mut().unwrap() += 1;
        }
    }
    (out, reads)
}

// ---------------------------------------------------------------------------------------------------------------
// one run of a scenario (chat_replay's `run_scenario`, without its comparisons with the recording)
// ---------------------------------------------------------------------------------------------------------------

struct Script {
    calls: Vec<Value>,
    gated: BTreeSet<usize>,
    supervise: Vec<Value>,
    systems: Vec<Value>,
    contexts: Vec<Value>,
    availability: String,
    spans: Value,
    note: Value,
    captures: Value,
    ports: Vec<Value>,
    front_door: bool,
    /// The front door's run only (journal 265): the recording's ids, so the stand-in writes the ids this run made in
    /// their place into recorded content (src/standins_chat.rs `Translation`); and the run's store, where it reads them.
    recorded_ids: Value,
    db: String,
}

/// The recording's ids for the stand-in's pairs (journal 265): each compared table's `id` column in row order, and the
/// journal's records (kind, data) in order.
fn recorded_ids(sc: &Value) -> Value {
    let mut tables = Map::new();
    for (t, v) in sc["tables"].as_object().into_iter().flatten() {
        let cols = strs(&v["columns"]);
        let Some(at) = cols.iter().position(|c| c == "id") else {
            continue;
        };
        if !compared(t) {
            continue;
        }
        let ids: Vec<Value> = v["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| r[at].clone())
            .collect();
        tables.insert(t.clone(), json!(ids));
    }
    let journal: Vec<Value> = sc["tables"]["journal"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| json!([cell(&r[3]), cell(&r[4])]))
        .collect();
    json!({"tables": tables, "journal": journal})
}

/// The recorded reads as the front door's run reads them ([`front_calls`]' counts): each planner call read its
/// `supervise` record, then its `system` record; each such pair is read as often as its count says (none for a repair
/// merged into the front door's run, twice for a hand-off's call: the front door's, then the planner's writing the
/// task). Every other record once.
fn reads_as(recs: &[Value], reads: &[usize]) -> Vec<Value> {
    let (mut out, mut nth, mut system_times) = (Vec::new(), 0, None);
    for r in recs {
        if r["kind"] == "supervise" {
            let times = reads.get(nth).copied().unwrap_or(1);
            nth += 1;
            system_times = Some(times);
            out.extend(std::iter::repeat_n(r.clone(), times));
            continue;
        }
        if r["kind"] == "system" {
            if let Some(times) = system_times.take() {
                out.extend(std::iter::repeat_n(r.clone(), times));
                continue;
            }
        }
        out.push(r.clone());
    }
    out
}

fn script_of(r: &Recorded, home: &Path, front: bool) -> Script {
    let workspace = &home.join("workspace").to_string_lossy().to_string();
    let (sc, catalogs) = (&r.sc, &r.catalogs);
    let recorded: Vec<Value> = sc["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(resolve)
        .collect();
    let (calls, recs) = if front {
        let (calls, reads) = front_calls(&recorded);
        (calls, reads_as(&r.recs, &reads))
    } else {
        (recorded, r.recs.clone())
    };
    let recs = &recs;
    let of = |k: &str| -> Vec<Value> { recs.iter().filter(|r| r["kind"] == k).cloned().collect() };
    let catalog_line = |v: &Value| -> String {
        let c = catalogs[s(&v["catalog"])]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ferb_py::json::dumps(&Value::Array(c))
    };
    let systems: Vec<Value> = of("system")
        .into_iter()
        .map(|mut r| {
            let skills = catalog_line(&r["inputs"]["skills_catalog"]);
            let abilities = catalog_line(&r["inputs"]["abilities_catalog"]);
            r["inputs"]["skills_line"] = json!(skills);
            r["inputs"]["abilities_line"] = json!(abilities);
            r
        })
        .collect();
    let availability = systems
        .first()
        .map(|r| s(&r["inputs"]["capabilities"]))
        .unwrap_or_default();
    let mut gated = BTreeSet::new();
    let mut last_wait = None;
    for st in sc["steps"].as_array().unwrap() {
        match st["do"].as_str().unwrap() {
            "wait" => last_wait = model_wait(&calls, &s(&st["until"])),
            "release_model" => {
                if let Some(i) = last_wait {
                    gated.insert(i);
                }
            }
            _ => {}
        }
    }
    let tables = &sc["tables"];
    let spans = json!({"columns": tables["spans"]["columns"], "rows": tables["spans"]["rows"]
        .as_array().cloned().unwrap_or_default().iter().map(|r| Value::Array(
            r.as_array().unwrap().iter().map(cell).collect())).collect::<Vec<_>>(), "used": []});
    let note = sc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|st| st["do"] == "note")
        .and_then(|st| {
            let text = s(&st["text"]);
            let rest = text.split_once("writes workspace/")?.1;
            let (path, rest) = rest.split_once(" (")?;
            let end = rest.rfind(')')?;
            let body: String = serde_json::from_str(&rest[..end]).ok()?;
            Some(json!([format!("{workspace}/{path}"), body]))
        })
        .unwrap_or(Value::Null);
    Script {
        calls,
        gated,
        supervise: of("supervise"),
        systems,
        contexts: of("context"),
        availability,
        spans,
        note,
        captures: sc["captures"].clone(),
        ports: of("port"),
        front_door: front,
        recorded_ids: if front { recorded_ids(sc) } else { Value::Null },
        db: home.join("ferb.db").to_string_lossy().to_string(),
    }
}

fn model_wait(calls: &[Value], until: &str) -> Option<usize> {
    let rest = until.strip_prefix("the model has received call ")?;
    let (key, n) = rest.rsplit_once(" (")?;
    let key = key.trim_matches('\'');
    let n: usize = n.trim_end_matches(')').parse().ok()?;
    // ORCH-743: the planner's call writing a hand-off's task is no recorded call of its own
    calls
        .iter()
        .enumerate()
        .filter(|(_, m)| m["key"] == key && m["handoff_planner"] != json!(true))
        .nth(n.checked_sub(1)?)
        .map(|(i, _)| i)
}

#[derive(Default)]
struct Used {
    made: Vec<Value>,
    started: Vec<Value>,
    tool_diffs: Vec<String>,
    supervise: usize,
    systems: usize,
    contexts: usize,
    spans: Vec<Value>,
    problems: Vec<String>,
    questions: u64,
    front_results: Vec<Value>,
}

impl Used {
    fn take(&mut self, st: &Value) {
        let all = |k: &str| st[k].as_array().cloned().unwrap_or_default();
        self.made.extend(all("made"));
        self.started.extend(all("started"));
        self.tool_diffs.extend(strs(&st["tool_diffs"]));
        self.supervise += st["supervise_used"].as_u64().unwrap_or(0) as usize;
        self.systems += st["systems_used"].as_u64().unwrap_or(0) as usize;
        self.contexts += st["contexts_used"].as_u64().unwrap_or(0) as usize;
        self.spans = all("spans_used");
        self.problems.extend(strs(&st["problems"]));
        self.questions = st["questions"].as_u64().unwrap_or(self.questions);
        self.front_results.extend(all("front_results"));
    }
}

fn write_script(path: &Path, sc: &Script, used: &Used, run: &RunRoot) {
    let mut spans = sc.spans.clone();
    spans["used"] = json!(used.spans);
    let file = json!({
        "calls": sc.calls, "first": used.made.len(), "gated": sc.gated,
        "supervise": sc.supervise[used.supervise.min(sc.supervise.len())..],
        "systems": sc.systems[used.systems.min(sc.systems.len())..],
        "contexts": sc.contexts[used.contexts.min(sc.contexts.len())..],
        "availability": sc.availability, "spans": spans, "root": run.root, "root_resolved": run.resolved,
        "note": sc.note, "questions_from": used.questions, "captures": sc.captures, "ports": sc.ports,
        "state_out": path.with_extension("state.json").to_string_lossy(),
        "front_door": sc.front_door, "recorded_ids": sc.recorded_ids, "db": sc.db,
    });
    std::fs::write(path, file.to_string()).unwrap();
}

fn config(script: &Path, ids: &str) -> Value {
    json!({"group_a": false, "ids": ids, "script": script.to_string_lossy(), "fs_identity": true,
           "programs": "recorder"})
}

fn settle(run: &Run) {
    let mut last = run.control(json!({"do": "quiet_mark"}));
    let mut quiet = 0;
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(15));
        let now = run.control(json!({"do": "quiet_mark"}));
        if now == last {
            quiet += 1;
            if quiet >= 12 {
                return;
            }
        } else {
            quiet = 0;
            last = now;
        }
    }
}

fn poll(run: &Run, secs: u64, mut done: impl FnMut(&Run) -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if done(run) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done(run)
}

fn recorded_start(run: &Run, sc: &Value, first_request: usize) {
    let mut statements = vec![(String::from("DELETE FROM journal"), Vec::<Value>::new())];
    for row in sc["tables"]["journal"]["rows"].as_array().unwrap() {
        let id = s(&row[1]);
        let n: usize = id
            .strip_prefix("id-")
            .and_then(|n| n.parse().ok())
            .unwrap_or(usize::MAX);
        if n < first_request {
            let vals: Vec<Value> = row.as_array().unwrap().iter().map(cell).collect();
            statements.push((
                "INSERT INTO journal (seq, id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, ?, ?)".into(),
                vals,
            ));
        }
    }
    run.control(json!({"do": "sql", "statements": statements}));
}

fn sql_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => json!(i),
        rusqlite::types::Value::Real(f) => json!(f),
        rusqlite::types::Value::Text(t) => json!(t),
        rusqlite::types::Value::Blob(b) => json!({"hex": hex::encode(b)}),
    }
}

/// chat_replay's `scrub`: the volatile keys (a process id, ...) at any depth, in JSON text too.
fn scrub(v: &Value) -> Value {
    const KEYS: [&str; 4] = ["pid", "max_sample_ms", "executable", "code_hash"];
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    if KEYS.contains(&k.as_str()) {
                        (k.clone(), json!("<volatile>"))
                    } else {
                        (k.clone(), scrub(x))
                    }
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(scrub).collect()),
        Value::String(t) => match serde_json::from_str::<Value>(t) {
            Ok(inner @ (Value::Object(_) | Value::Array(_))) => {
                let masked = scrub(&inner);
                if masked == inner {
                    v.clone()
                } else {
                    json!(ferb_py::json::dumps_utf8(&masked))
                }
            }
            _ => v.clone(),
        },
        other => other.clone(),
    }
}

/// Every SHA-256 in a text as `<sha256>` (chat_replay's `unhash`): a hash over a path under the run's own folder (a
/// ledger row's payload, a confirmation's key) differs with the folder.
fn unhash(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let run = b[i..]
            .iter()
            .take_while(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
            .count();
        if run >= 64 {
            let whole = run / 64 * 64;
            out.push_str(&"<sha256>".repeat(whole / 64));
            out.push_str(&text[i + whole..i + run]);
            i += run;
        } else if run > 0 {
            out.push_str(&text[i..i + run]);
            i += run;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

type Tables = BTreeMap<String, (Vec<String>, Vec<Vec<Value>>)>;

fn dump(db: &Path, run: &RunRoot) -> Tables {
    let conn = rusqlite::Connection::open(db).unwrap();
    let names: Vec<String> = {
        let mut st = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut out = BTreeMap::new();
    for name in names {
        if name.starts_with("sqlite_") || name == "schema_version" || name == "schema_migrations" {
            continue;
        }
        let Ok(mut st) = conn.prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid")) else {
            continue;
        };
        let cols: Vec<String> = st.column_names().map(|c| c.to_string()).collect();
        let n = cols.len();
        let rows: Vec<Vec<Value>> = match st.query_map([], |r| {
            (0..n)
                .map(|i| r.get::<_, rusqlite::types::Value>(i).map(sql_json))
                .collect::<Result<Vec<_>, _>>()
        }) {
            Ok(rows) => rows.map(Result::unwrap).collect(),
            Err(_) => continue,
        };
        let rows = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|v| match v {
                        Value::String(t) => scrub(&json!(unhash(&run.mask(&t)))),
                        other => other,
                    })
                    .collect()
            })
            .collect();
        out.insert(name, (cols, rows));
    }
    out
}

/// What one run of a scenario left: the store, every chat stream's frames (by connection), the scripted model's use.
struct Outcome {
    tables: Tables,
    streams: Vec<(String, Vec<Value>)>,
    used: Used,
    stopped: Option<String>,
}

fn run_scenario(r: &Recorded, run_root: &RunRoot, front: bool) -> Outcome {
    let sc = &r.sc;
    let nn = format!("{:02}", r.index + 1);
    let root = PathBuf::from(&run_root.root);
    let (home, user) = Run::paths(&root, &nn);
    let _ = std::fs::remove_dir_all(root.join(&nn));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let script = script_of(r, &home, front);
    let script_path = root.join(format!("{nn}-script.json"));
    let mut used = Used::default();
    write_script(&script_path, &script, &used, run_root);
    let mut run = Run::start(&root, &nn, config(&script_path, "id"));
    let first_request: usize = sc["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|st| st["lines"].as_array())
        .flatten()
        .filter_map(|l| l.get("recv"))
        .map(line_of)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .filter(|v| v["params"]["type"] == "ack")
        .filter_map(|v| {
            s(&v["params"]["request_id"])
                .get(3..)
                .and_then(|n| n.parse().ok())
        })
        .min()
        .unwrap_or(15);
    recorded_start(&run, sc, first_request);
    run.control(json!({"do": "ids_next", "n": first_request}));
    let first_token = sc["tables"]["timers"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r[8].as_str())
        .filter_map(|t| t.trim_start_matches('t').parse::<u64>().ok())
        .min()
        .unwrap_or(1);
    run.control(json!({"do": "tokens", "n": first_token.saturating_sub(1)}));

    let mut conns: BTreeMap<String, Raw> = BTreeMap::new();
    let mut ids: BTreeMap<String, Value> = BTreeMap::new();
    let mut closed: BTreeMap<String, Raw> = BTreeMap::new();
    let mut streams: Vec<(String, Vec<Value>)> = Vec::new();
    let mut stopped = None;
    let frame =
        |l: &str| -> Value { serde_json::from_str(&run_root.mask(l)).unwrap_or_else(|_| json!(l)) };
    // the front door's run (journal 265): a recorded line the owner sends names the ids this run made in place of the
    // recording's (a timer the app lists and then repeats, ...)
    let sent_line = |run: &Run, l: &Value| -> String {
        let line = line_of(l);
        if !front {
            return line;
        }
        let got = run.control(json!({"do": "translate", "value": line}));
        got.as_str().map(str::to_string).unwrap_or(line)
    };
    // and so does a recorded event or tap that names a task (ORCH-743: the planner's call after a hand-off writes
    // records of its own, so the task's id here is not the recording's)
    let sent_value = |run: &Run, v: &Value| -> Value {
        if !front {
            return v.clone();
        }
        run.control(json!({"do": "translate", "value": v}))
    };
    for st in sc["steps"].as_array().unwrap() {
        let conn = s(&st["conn"]);
        match st["do"].as_str().unwrap() {
            "connect" => {
                conns.insert(conn, Raw::connect(&run.socket()).unwrap());
            }
            "seed" => {}
            "write_file" => {
                let path = home.join(s(&st["path"]));
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).unwrap();
                }
                std::fs::write(&path, s(&st["text"])).unwrap();
            }
            "close" => {
                if let Some(c) = conns.remove(&conn) {
                    if ids.contains_key(&conn) {
                        closed.insert(conn, c);
                    }
                }
            }
            "call" => {
                let c = conns.get_mut(&conn).expect("an open connection");
                let mut got = Vec::new();
                for l in st["lines"].as_array().unwrap() {
                    if let Some(sent) = l.get("send") {
                        c.send(&sent_line(&run, sent));
                    } else if l.get("recv").is_some() {
                        got.push(c.recv().map(|g| frame(&g)).unwrap_or(json!("(closed)")));
                    }
                }
                streams.push((format!("{conn} call"), got));
                settle(&run);
            }
            "stream" => {
                let sent = sent_line(&run, &st["lines"][0]["send"]);
                let v: Value = serde_json::from_str(&sent).unwrap();
                ids.insert(conn.clone(), v["id"].clone());
                conns
                    .get_mut(&conn)
                    .expect("an open connection")
                    .send(&sent);
            }
            "stream_end" | "stream_end_at_shutdown" | "stream_cut" => {
                let want = st["lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l.get("recv").is_some())
                    .count();
                let Some(c) = conns.get_mut(&conn).or_else(|| closed.get_mut(&conn)) else {
                    stopped = Some(format!("{conn}'s stream, never opened here"));
                    break;
                };
                let mut got = Vec::new();
                if st["do"] == "stream_cut" {
                    for _ in 0..want {
                        match c.recv() {
                            Some(l) => got.push(frame(&l)),
                            None => break,
                        }
                    }
                    conns.remove(&conn);
                    closed.remove(&conn);
                } else {
                    let id = ids.get(&conn).cloned().unwrap_or(Value::Null);
                    while let Some(l) = c.recv() {
                        let v = frame(&l);
                        let end = v["id"] == id;
                        got.push(v);
                        if end {
                            break;
                        }
                    }
                }
                streams.push((conn.clone(), got));
            }
            "wait" => {
                let until = s(&st["until"]);
                let ok = if let Some(i) = model_wait(&script.calls, &until) {
                    poll(&run, 30, |r| {
                        let state = r.control(json!({"do": "script"}));
                        state["started"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|c| c["n"] == json!(i)))
                    })
                } else if until.starts_with("an approval is pending") {
                    poll(&run, 30, |r| {
                        r.control(json!({"do": "pending"})) != json!(0)
                    })
                } else if until == "the task is paused"
                    || until == "the task ended"
                    || until == "the task is done"
                {
                    let paused = until == "the task is paused";
                    let done = until == "the task is done";
                    poll(&run, 30, |r| {
                        match r.control(json!({"do": "root_status"})).as_str() {
                            Some("paused") => paused,
                            Some("running") | Some("planned") | None => false,
                            Some("done") => !paused,
                            Some(_) => !paused && !done,
                        }
                    })
                } else if until.starts_with("the probe has read ") {
                    std::thread::sleep(Duration::from_millis(500));
                    true
                } else if until == "the loop watches" {
                    poll(&run, 30, |r| {
                        r.control(json!({"do": "loop_watching"})) == json!(true)
                    })
                } else if until == "the conversation waits" {
                    poll(&run, 30, |r| {
                        r.control(json!({"do": "conversation_waits"})) == json!(true)
                    })
                } else if until == "the plan waits" {
                    poll(&run, 30, |r| {
                        r.control(json!({"do": "plan_waits"})) == json!(true)
                    })
                } else if until == "the worker's wait.for is running" {
                    poll(&run, 30, |r| {
                        r.control(json!({"do": "sleeping"})) != json!(0)
                    })
                } else if until == "the first checker has answered" {
                    let first = script
                        .calls
                        .iter()
                        .position(|m| m["key"] == "worker.verify");
                    poll(&run, 30, |r| {
                        let state = r.control(json!({"do": "script"}));
                        first.is_some_and(|f| {
                            state["answered"]
                                .as_array()
                                .is_some_and(|a| a.contains(&json!(f)))
                        })
                    })
                } else {
                    false
                };
                if !ok {
                    stopped = Some(format!("a wait for {until} (never true here)"));
                    break;
                }
                settle(&run);
            }
            "release_model" => {
                run.control(json!({"do": "release_model"}));
                settle(&run);
            }
            "mention_paused" => {
                run.control(json!({"do": "mention_paused"}));
                settle(&run);
            }
            "preview" => {
                let got = run.control(json!({"do": "preview", "session_id": st["session_id"],
                                             "text": st["text"]}));
                streams.push(("preview".into(), vec![got]));
                settle(&run);
            }
            "attention" => {
                let got = run.control(json!({"do": "attention", "source_refs": st["source_refs"],
                                             "observations": st["observations"],
                                             "already_told": st["already_told"]}));
                streams.push(("attention".into(), vec![got]));
                settle(&run);
            }
            "plan_event" => {
                let data = sent_value(&run, &st["data"]);
                run.control(json!({"do": "plan_event", "topic": st["topic"], "data": data}));
                settle(&run);
            }
            "runtime_switch" => {
                let task_id = sent_value(&run, &st["task_id"]);
                let got = run.control(json!({"do": "runtime_switch", "task_id": task_id,
                                             "runtime": st["runtime"]}));
                streams.push(("runtime_switch".into(), vec![got]));
                settle(&run);
            }
            "note" => {
                run.control(json!({"do": "note"}));
            }
            "clock" => {
                run.control(json!({"do": "clock", "advance_s": st["advance_s"]}));
                settle(&run);
            }
            "timers_tick" => {
                let fired = run.control(json!({"do": "timers_tick"}));
                streams.push(("timers_tick".into(), vec![fired]));
                settle(&run);
            }
            "restart" => {
                let (status, err) = run.stop();
                if !status.success() {
                    stopped = Some(format!("the daemon's shutdown: {status} {err}"));
                    break;
                }
                let state: Value = serde_json::from_str(
                    &std::fs::read_to_string(script_path.with_extension("state.json"))
                        .unwrap_or_default(),
                )
                .unwrap_or(Value::Null);
                used.take(&state);
                write_script(&script_path, &script, &used, run_root);
                std::fs::write(
                    &run.stand,
                    {
                        let mut c = config(&script_path, &s(&st["ids_prefix"]));
                        c["user_home"] = json!(user.to_string_lossy());
                        c
                    }
                    .to_string(),
                )
                .unwrap();
                run.spawn();
                settle(&run);
            }
            other => {
                stopped = Some(format!("the step {other:?}"));
                break;
            }
        }
    }
    settle(&run);
    used.take(&run.control(json!({"do": "script"})));
    run.control(json!({"do": "chat_flush"}));
    let tables = dump(&home.join("ferb.db"), run_root);
    drop(conns);
    let _ = run.stop();
    Outcome {
        tables,
        streams,
        used,
        stopped,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the comparison
// ---------------------------------------------------------------------------------------------------------------

/// Tables not compared: the records of how a turn was decided rather than what it did (the model calls' and tool
/// calls' spans, the planner's or front door's journal rows are compared apart in [`journal_view`]), the learning
/// part's derived rows, the perception floor, and stage 2's chat entries (compared through the streams' frames).
fn compared(table: &str) -> bool {
    ![
        "spans",
        "journal",
        "perception_floor",
        "recall_state",
        "trail_changes",
    ]
    .contains(&table)
        && !ferb_store::chat_entries::TABLES.contains(&table)
        && !table.starts_with("learning_")
        && !table.contains("_fts")
        && !["_data", "_idx", "_content", "_docsize", "_config"]
            .iter()
            .any(|x| table.ends_with(x))
}

/// The journal's kinds that are how a turn was decided, not its effect: the model's calls and the front door's own
/// records of refused calls. Every other kind is compared, record by record.
const JOURNAL_DECIDING: [&str; 4] = ["model.usage", "task.repair", "runtime.usage", "model.close"];

/// Ids numbered by their first appearance (`<prefix>-<6 digits>`, a counter's), so two runs that drew a different
/// number of ids along the way compare equal where they did the same.
struct Renumber(BTreeMap<String, String>);

impl Renumber {
    fn text(&mut self, t: &str) -> String {
        let b = t.as_bytes();
        let mut out = String::with_capacity(t.len());
        let mut i = 0;
        while i < b.len() {
            // a run of [a-z0-9] ending in "-" and six digits, not inside a longer word
            if (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && b[i].is_ascii_alphanumeric() {
                let start = i;
                let mut j = i;
                while j < b.len() && b[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                if j + 7 <= b.len()
                    && b[j] == b'-'
                    && b[j + 1..j + 7].iter().all(u8::is_ascii_digit)
                    && (j + 7 == b.len() || !b[j + 7].is_ascii_alphanumeric())
                {
                    let id = &t[start..j + 7];
                    let n = self.0.len();
                    let named = self
                        .0
                        .entry(id.to_string())
                        .or_insert_with(|| format!("ID{n}"))
                        .clone();
                    out.push_str(&named);
                    i = j + 7;
                    continue;
                }
                out.push_str(&t[start..j]);
                i = j;
                continue;
            }
            let ch = t[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    fn value(&mut self, v: &Value) -> Value {
        match v {
            Value::String(t) => json!(self.text(t)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), self.value(x))).collect())
            }
            other => other.clone(),
        }
    }
}

/// The journal's records of what happened (not how the turn was decided): (kind, data), ids renumbered.
fn journal_view(o: &Outcome, ids: &mut Renumber) -> Vec<Value> {
    let Some((cols, rows)) = o.tables.get("journal") else {
        return Vec::new();
    };
    let (k, d) = (
        cols.iter().position(|c| c == "kind").unwrap(),
        cols.iter().position(|c| c == "data_json").unwrap(),
    );
    rows.iter()
        .filter(|r| !JOURNAL_DECIDING.contains(&s(&r[k]).as_str()))
        .map(|r| {
            let data: Value = serde_json::from_str(&s(&r[d])).unwrap_or(r[d].clone());
            json!([r[k], ids.value(&data)])
        })
        .collect()
}

/// A chat stream's frames by kind, as the app draws them: each frame's type (the plan channel's, the reply's, a task's
/// events), a text frame's stream of pieces counted once, and the stream's end; its words are compared in the store.
fn frame_kinds(frames: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in frames {
        let kind = if f.get("method").is_some() {
            s(&f["params"]["type"])
        } else if f.get("result").is_some() {
            "(end)".into()
        } else {
            "(other)".into()
        };
        if out.last() != Some(&kind) {
            out.push(kind);
        }
    }
    out
}

/// Every model call FERB made after the first (the worker's, the checker's, ...) as compared: role, mode, tools,
/// schema, its task, its instructions and its input (ids renumbered); the planner's own calls (the front door's are
/// made with no form and its own tools and instructions) are left out.
fn later_calls(o: &Outcome, ids: &mut Renumber) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, made) in o.used.made.iter().enumerate() {
        if made["runtime_role"] == "orchestrator" {
            continue;
        }
        let started = o.used.started.iter().find(|c| c["n"] == json!(i));
        let started = started.cloned().unwrap_or(Value::Null);
        let started: Value = serde_json::from_str(&unhash(&started.to_string())).unwrap();
        out.push(ids.value(
            &json!({"runtime_role": made["runtime_role"], "mode": made["mode"],
            "tools": made["tools"], "schema": made["schema"], "effort": made["effort"],
            "task_id": started["task_id"], "system": started["system"], "parts": started["parts"]}),
        ));
    }
    out
}

/// Each difference between the planner's run (`old`) and the front door's (`front`): (area, detail).
fn differences(old: &Outcome, front: &Outcome) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(stop) = &front.stopped {
        out.push(("stopped".into(), stop.clone()));
    }
    // the old path is chat_replay's run of the recording: its own script problems would make this comparison moot
    // (ORCH-648's named difference, as chat_replay names it: login_sites asks the browser area once more)
    let old_problems: Vec<&String> = old
        .used
        .problems
        .iter()
        .filter(|p| p.as_str() != "port browsing: not called in the recording")
        .collect();
    if old.stopped.is_some() || !old_problems.is_empty() || !old.used.tool_diffs.is_empty() {
        out.push((
            "old path".into(),
            format!(
                "the planner's run didn't replay: {:?} {:?} {:?}",
                old.stopped, old.used.problems, old.used.tool_diffs
            ),
        ));
    }
    // a recorded tool call's reply is compared as the stand-in wrote it, with the ids this run made (journal 265)
    for (what, a, b) in [
        ("problems", &old.used.problems, &front.used.problems),
        ("tool calls", &old.used.tool_diffs, &front.used.tool_diffs),
    ] {
        if a != b {
            out.push((
                what.into(),
                format!("planner: {a:?}\n    front door: {b:?}"),
            ));
        }
    }
    for r in &front.used.front_results {
        if r[3]["is_error"] != json!(false) {
            out.push((
                format!("refused {}", s(&r[1])),
                format!("{}", r[3]["content"]),
            ));
        }
    }
    // ids are numbered afresh for each table and each other part compared, so a record one run has and the other
    // hasn't moves no numbers elsewhere
    let fresh = || (Renumber(BTreeMap::new()), Renumber(BTreeMap::new()));
    // the store's tables, in name order, rows in order
    let names: BTreeSet<&String> = old.tables.keys().chain(front.tables.keys()).collect();
    for t in names {
        if !compared(t) {
            continue;
        }
        let empty = (Vec::new(), Vec::new());
        let (ac, ar) = old.tables.get(t).unwrap_or(&empty);
        let (bc, br) = front.tables.get(t).unwrap_or(&empty);
        let (mut a_ids, mut b_ids) = fresh();
        let ar: Vec<Value> = ar.iter().map(|r| a_ids.value(&json!(r))).collect();
        let br: Vec<Value> = br.iter().map(|r| b_ids.value(&json!(r))).collect();
        if ar.len() != br.len() {
            out.push((
                format!("{t} rows"),
                format!("planner {} rows, front door {}", ar.len(), br.len()),
            ));
            continue;
        }
        let cols = if ac.is_empty() { bc } else { ac };
        for (i, (x, y)) in ar.iter().zip(&br).enumerate() {
            if x == y {
                continue;
            }
            for c in (0..cols.len()).filter(|c| x[*c] != y[*c]) {
                out.push((
                    format!("{t}.{}", cols[c]),
                    format!(
                        "row {i}\n    planner:    {}\n    front door: {}",
                        x[c], y[c]
                    ),
                ));
            }
        }
    }
    let (mut a_ids, mut b_ids) = fresh();
    let (ja, jb) = (
        journal_view(old, &mut a_ids),
        journal_view(front, &mut b_ids),
    );
    let kinds = |j: &[Value]| -> Vec<String> { j.iter().map(|r| s(&r[0])).collect() };
    if kinds(&ja) != kinds(&jb) {
        out.push((
            "journal kinds".into(),
            format!(
                "planner:    {:?}\n    front door: {:?}",
                kinds(&ja),
                kinds(&jb)
            ),
        ));
    } else {
        for (x, y) in ja.iter().zip(&jb) {
            if x != y {
                out.push((
                    format!("journal {}", s(&x[0])),
                    format!("planner:    {}\n    front door: {}", x[1], y[1]),
                ));
            }
        }
    }
    let (mut a_ids, mut b_ids) = fresh();
    let (ca, cb) = (later_calls(old, &mut a_ids), later_calls(front, &mut b_ids));
    if ca.len() != cb.len() {
        out.push((
            "later model calls".into(),
            format!("planner {}, front door {}", ca.len(), cb.len()),
        ));
    }
    for (i, (x, y)) in ca.iter().zip(&cb).enumerate() {
        if x != y {
            let field = x
                .as_object()
                .unwrap()
                .keys()
                .find(|k| x[*k] != y[*k])
                .cloned()
                .unwrap_or_default();
            let (a, b) = (x[&field].to_string(), y[&field].to_string());
            out.push((
                format!("later model call {field}"),
                format!("call {i} {}", first_diff(&a, &b)),
            ));
        }
    }
    let streams = |o: &Outcome| -> Vec<(String, Vec<String>)> {
        o.streams
            .iter()
            .map(|(c, f)| (c.clone(), frame_kinds(f)))
            .collect()
    };
    let (sa, sb) = (streams(old), streams(front));
    for (i, (x, y)) in sa.iter().zip(&sb).enumerate() {
        if x == y {
            continue;
        }
        // named by the kinds only one side has, else by their order
        let only = |a: &[String], b: &[String]| -> Vec<String> {
            a.iter().filter(|k| !b.contains(k)).cloned().collect()
        };
        let (more, fewer) = (only(&y.1, &x.1), only(&x.1, &y.1));
        let area = if more.is_empty() && fewer.is_empty() {
            "frames order".to_string()
        } else {
            let mut parts: Vec<String> = more.iter().map(|k| format!("+{k}")).collect();
            parts.extend(fewer.iter().map(|k| format!("-{k}")));
            format!("frames {}", parts.join(" "))
        };
        out.push((
            area,
            format!(
                "stream {i} ({})\n    planner:    {:?}\n    front door: {:?}",
                x.0, x.1, y.1
            ),
        ));
    }
    if sa.len() != sb.len() {
        out.push((
            "frames".into(),
            format!("{} streams, {}", sa.len(), sb.len()),
        ));
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// the expected differences, each with its reason
// ---------------------------------------------------------------------------------------------------------------

/// Every difference the front door makes on the recorded scenarios: (the difference's area, the scenarios it is found
/// in, why). A difference found in another scenario fails the test; so does one named here that a scenario run no longer
/// shows.
const EXPECTED: &[(&str, &[&str], &str)] = &[
    (
        "frames +reply_delta",
        &["plain_reply", "pause_steer_resume", "steer_running_task", "global_stop", "schedule_created_and_fired", "timer_set_and_fired", "summary_fold", "remember_then_forget", "answer_in_chat", "loop_steer_finish", "mention_paused", "capture_timeout", "conversation_change", "stated_scope", "login_sites", "displaced_question"],
        "The front door streams its plain reply as written (RUN-480, ORCH-734): reply_delta frames before the reply. The recorded planner gave its whole answer at once, so the planner's replay streams none (a real model's form streams through ReplyDraft).",
    ),
    (
        "episodes.tools_json",
        &["clarify_then_answer", "one_step_task", "one_step_with_tools", "checker_skips_change_records", "multi_step_plan", "confirm_no", "confirm_stop_pending", "steer_running_task", "stop_by_task_stop", "stop_by_chat", "stop_plus_new_task", "global_stop", "schedule_created_and_fired", "wait_for_time", "tool_failure_then_retry", "plan_repair", "checker_rejects", "checker_findings_fix", "worker_question_answered", "check_in_after_five", "restart_mid_task", "restart_with_open_question", "attachment_and_bad_params", "several_asks", "plan_parts_checklist", "answer_in_chat", "checklist_single", "ask_step", "wait_step", "route_question", "checker_runtime_down", "unbacked_part", "correct_pending_check", "loop_steer_finish", "loop_pause_stop", "mention_paused", "owner_closed_tab", "reason_findings", "runtime_switch_tap", "event_wait", "conversation_handover", "conversation_change", "stated_scope", "browse_as_corrected", "probe_watch", "login_sites", "conversation_resume", "confirm_yes", "pause_steer_resume", "earlier_result"],
        "The front door's calls are gateway tool calls, so the episode counts them ({\"chat.dispatch_task\": {\"calls\": 1, ...}}); the planner's form was no tool call.",
    ),
    (
        "episodes.tool_origins",
        &["clarify_then_answer", "one_step_task", "multi_step_plan", "confirm_no", "steer_running_task", "stop_by_task_stop", "stop_by_chat", "stop_plus_new_task", "global_stop", "schedule_created_and_fired", "tool_failure_then_retry", "plan_repair", "checker_rejects", "checker_findings_fix", "worker_question_answered", "check_in_after_five", "restart_mid_task", "restart_with_open_question", "attachment_and_bad_params", "several_asks", "plan_parts_checklist", "answer_in_chat", "checklist_single", "ask_step", "wait_step", "route_question", "checker_runtime_down", "unbacked_part", "loop_steer_finish", "loop_pause_stop", "mention_paused", "owner_closed_tab", "reason_findings", "runtime_switch_tap", "event_wait", "conversation_handover", "conversation_change", "stated_scope", "browse_as_corrected", "probe_watch", "login_sites", "conversation_resume", "earlier_result"],
        "The front door's call results are FERB's own words (origin ferb), which join the episode's tool origins.",
    ),
    (
        "episodes.usage_json",
        &["clarify_then_answer", "one_step_task", "one_step_with_tools", "checker_skips_change_records", "multi_step_plan", "confirm_no", "confirm_stop_pending", "steer_running_task", "stop_by_task_stop", "stop_by_chat", "stop_plus_new_task", "global_stop", "schedule_created_and_fired", "wait_for_time", "tool_failure_then_retry", "plan_repair", "checker_rejects", "checker_findings_fix", "worker_question_answered", "check_in_after_five", "restart_mid_task", "restart_with_open_question", "attachment_and_bad_params", "several_asks", "plan_parts_checklist", "answer_in_chat", "checklist_single", "ask_step", "wait_step", "route_question", "checker_runtime_down", "unbacked_part", "correct_pending_check", "loop_steer_finish", "loop_pause_stop", "mention_paused", "owner_closed_tab", "reason_findings", "runtime_switch_tap", "event_wait", "conversation_handover", "conversation_change", "stated_scope", "browse_as_corrected", "probe_watch", "login_sites", "conversation_resume", "confirm_yes", "pause_steer_resume", "earlier_result"],
        "A run ended on its hand-off or question (ORCH-733) reads no further event, so the scripted usage event after its calls is never read: one usage record fewer. Whether a real runtime's usage reaches the record at its close (RUN-477) is unverified here.",
    ),
    (
        "frames +plan_delta +plan_end +progress",
        &["clarify_then_answer", "one_step_task", "multi_step_plan", "steer_running_task", "stop_by_task_stop", "stop_by_chat", "stop_plus_new_task", "global_stop", "checker_rejects", "checker_findings_fix", "worker_question_answered", "check_in_after_five", "restart_mid_task", "restart_with_open_question", "attachment_and_bad_params", "several_asks", "plan_parts_checklist", "answer_in_chat", "checklist_single", "ask_step", "checker_runtime_down", "earlier_result", "loop_steer_finish", "loop_pause_stop", "mention_paused", "owner_closed_tab", "reason_findings", "runtime_switch_tap", "stated_scope", "browse_as_corrected", "login_sites"],
        "An accepted hand-off's ack is shown on the plan channel (ack, ack_end) before its plan frame (ORCH-735), and a planning line runs under it while the planner writes the task (ORCH-747); the recorded planner's whole answer streamed nothing first.",
    ),
    (
        "frames +plan_delta +plan_end",
        &["checker_skips_change_records", "confirm_no", "confirm_stop_pending", "confirm_yes", "conversation_change", "conversation_handover", "conversation_resume", "correct_pending_check", "event_wait", "one_step_with_tools", "pause_steer_resume", "probe_watch", "route_question", "tool_failure_then_retry", "wait_for_time", "wait_step"],
        "An accepted hand-off's ack is shown on the plan channel (ack, ack_end) before its plan frame (ORCH-735), and a planning line runs under it while the planner writes the task (ORCH-747; these runs already had a live line later, so it shows as no new kind); the recorded planner's whole answer streamed nothing first.",
    ),
    (
        "episodes.steps_json",
        &["answer_in_chat", "ask_step", "attachment_and_bad_params", "browse_as_corrected", "check_in_after_five", "checker_findings_fix", "checker_rejects", "checker_runtime_down", "checker_skips_change_records", "checklist_single", "clarify_then_answer", "confirm_no", "confirm_stop_pending", "confirm_yes", "conversation_change", "conversation_handover", "conversation_resume", "correct_pending_check", "earlier_result", "event_wait", "global_stop", "login_sites", "loop_pause_stop", "loop_steer_finish", "mention_paused", "multi_step_plan", "one_step_task", "one_step_with_tools", "owner_closed_tab", "pause_steer_resume", "plan_parts_checklist", "probe_watch", "reason_findings", "restart_mid_task", "restart_with_open_question", "route_question", "runtime_switch_tap", "schedule_created_and_fired", "several_asks", "stated_scope", "steer_running_task", "stop_by_chat", "stop_by_task_stop", "stop_plus_new_task", "tool_failure_then_retry", "wait_for_time", "wait_step", "worker_question_answered", "plan_repair", "unbacked_part"],
        "A hand-off's turn makes two planner calls (ORCH-743): the front door's, whose Planner line is its ack (\"task — On it.\"), then the planner's writing the task, whose line is the planner run's (\"task — <goal>\"); so the episode has one Planner line more, and each later line's \"verified in step N\" counts one further. In plan_repair and unbacked_part the planner's run repaired its first answer (two Planner lines); the front door hands off at once and the planner writing the task repairs it the same way (three).",
    ),
    (
        "frames +plan_delta +plan_end +progress -plan_abort",
        &["plan_repair", "unbacked_part"],
        "The planner's run showed the refused plan and withdrew it (plan_abort) before the repaired one; the front door shows a hand-off only once code accepted it (ack, ack_end).",
    ),
    (
        "refused chat.control_task",
        &["conversation_change"],
        "The recorded answer pauses the chat in the owner's words and closes it quoting words he never wrote, and its repair repeats them. As in the planner's run, the pause is applied and the close refused (ORCH-739: the call keeps its good entry and its error names the refused one), so the call shows as refused in the trace.",
    ),
    (
        "frames +plan_delta +plan_end -plan_abort",
        &["inventory_walk"],
        "The front door hands off at once (its ack shown: plan_delta, plan_end); the planner writing the task then gives a plan code refuses and repairs it into a reply, which stands (ORCH-743: a planner that replies instead is heard). That planner call has no live stream, so the owner sees neither the walk of the plan's folders (progress) nor the plan's withdrawal (plan_abort): the ack, then the reply.",
    ),
];

/// How many tests the scenarios are split into (each one under the test guard's 300 s, TEST-511).
const BATCHES: usize = 8;

/// The scenarios run through the front door, in the recordings' order, and the ones not run, each with why (read once
/// for every batch: one copy of the recordings in this process).
fn eligible() -> &'static (Vec<Recorded>, Vec<String>) {
    static E: OnceLock<(Vec<Recorded>, Vec<String>)> = OnceLock::new();
    E.get_or_init(|| {
        let (mut run, mut skips) = (Vec::new(), Vec::new());
        for r in recorded() {
            match not_today_s_form(&r.sc) {
                Some(why) => skips.push(format!("{}: {why}", s(&r.sc["name"]))),
                None => run.push(r),
            }
        }
        (run, skips)
    })
}

/// How many scenarios run at once across the batch tests, each as two daemons side by side. Under ferb-test-guard
/// (TEST-511) the binary's processes together stay under 4096 MB: measured 2026-11-27, a scenario's two daemons and
/// their share of this process take about 260 MB each, so eight scenarios at once (all batches) peaked at 4225 MB.
const AT_ONCE: usize = 5;

/// A place among the [`AT_ONCE`] scenarios running, held until dropped.
struct Place;

fn places() -> &'static (std::sync::Mutex<usize>, std::sync::Condvar) {
    static P: OnceLock<(std::sync::Mutex<usize>, std::sync::Condvar)> = OnceLock::new();
    P.get_or_init(|| (std::sync::Mutex::new(0), std::sync::Condvar::new()))
}

impl Place {
    fn take() -> Place {
        let (n, freed) = places();
        let mut n = n.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= AT_ONCE {
            n = freed.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Place
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let (n, freed) = places();
        *n.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        freed.notify_one();
    }
}

/// One scenario through both paths: each difference (area, detail).
fn compare(r: &Recorded, len: usize, debug: bool) -> Vec<(String, String)> {
    let _place = Place::take();
    let (a, old_root) = RunRoot::new(len);
    let (b, front_root) = RunRoot::new(len);
    // FRONT_REPLAY_KEEP: both runs' folders stay, for a look at what the daemon left
    let _kept = if std::env::var("FRONT_REPLAY_KEEP").is_ok() {
        eprintln!("kept: {} {}", a.keep().display(), b.keep().display());
        None
    } else {
        Some((a, b))
    };
    // the two runs side by side, each its own program on its own folder
    let (old, front) = std::thread::scope(|sc| {
        let a = sc.spawn(|| run_scenario(r, &old_root, false));
        let b = sc.spawn(|| run_scenario(r, &front_root, true));
        (a.join().unwrap(), b.join().unwrap())
    });
    if debug {
        for (what, o) in [("planner", &old), ("front door", &front)] {
            let made: Vec<String> = o
                .used
                .made
                .iter()
                .zip(
                    o.used
                        .started
                        .iter()
                        .map(Some)
                        .chain(std::iter::repeat(None)),
                )
                .map(|(m, st)| {
                    format!(
                        "{}/{} {}",
                        s(&m["runtime_role"]),
                        s(&m["mode"]),
                        st.map(|st| s(&st["task_id"])).unwrap_or_default()
                    )
                })
                .collect();
            let calls: Vec<String> = o
                .used
                .front_results
                .iter()
                .map(|c| format!("{} {}", s(&c[1]), c[3]["content"]))
                .collect();
            eprintln!("  ({what} made: {made:?}; front calls: {calls:?})");
        }
    }
    if std::env::var("FRONT_REPLAY_IDS").is_ok() {
        for (what, o) in [("planner", &old), ("front door", &front)] {
            let mut ids: Vec<(String, String)> = Vec::new();
            for t in ["spans", "journal", "tasks"] {
                if let Some((cols, rows)) = o.tables.get(t) {
                    let id = cols.iter().position(|c| c == "id").unwrap_or(0);
                    let kind = cols
                        .iter()
                        .position(|c| c == "kind" || c == "goal")
                        .unwrap_or(0);
                    for r in rows {
                        ids.push((s(&r[id]), format!("{t}:{}", s(&r[kind]))));
                    }
                }
            }
            ids.sort();
            eprintln!("  {what} ids: {ids:?}");
        }
    }
    differences(&old, &front)
}

/// Batch `k` of [`BATCHES`] (`FRONT_REPLAY=<name>` runs that one scenario in whichever batch holds it): each scenario
/// through both paths, every difference checked against [`EXPECTED`].
fn batch(k: usize) {
    let chat = fixture("daemon/chat_scenarios.json");
    let len = recorded_root_len(&chat);
    let only = std::env::var("FRONT_REPLAY").ok();
    let debug = std::env::var("FRONT_REPLAY_DEBUG").is_ok();
    let (all, skips) = eligible();
    if k == 0 {
        eprintln!(
            "{} scenarios through the front door; {} not: {skips:#?}",
            all.len(),
            skips.len()
        );
    }
    let size = all.len().div_ceil(BATCHES);
    let mut found: Vec<(String, String, String)> = Vec::new();
    let mut ran: Vec<String> = Vec::new();
    for r in all.iter().skip(k * size).take(size) {
        let name = s(&r.sc["name"]);
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let diffs = compare(r, len, debug);
        eprintln!("{name}: {} differences", diffs.len());
        for (area, detail) in diffs {
            if debug {
                eprintln!("  {area}: {detail}");
            }
            found.push((name.clone(), area, detail));
        }
        ran.push(name);
    }
    let named = |n: &str, area: &str| {
        EXPECTED
            .iter()
            .any(|(a, scs, _)| *a == area && scs.contains(&n))
    };
    let mut unnamed: Vec<String> = Vec::new();
    for (n, a, d) in &found {
        let line = format!("{n}: {a}: {}", if debug { d.clone() } else { cut(d) });
        if !named(n, a) && !unnamed.contains(&line) {
            unnamed.push(line);
        }
    }
    let mut gone: Vec<String> = Vec::new();
    for (area, scs, _) in EXPECTED {
        for sc in scs.iter().filter(|sc| ran.iter().any(|n| n == *sc)) {
            if !found.iter().any(|(n, a, _)| n == sc && a == area) {
                gone.push(format!("{sc}: {area} (named, no longer found)"));
            }
        }
    }
    assert!(unnamed.is_empty(), "unnamed differences: {unnamed:#?}");
    assert!(gone.is_empty(), "named differences not found: {gone:#?}");
}

/// Every named difference names a scenario that is run, and every scenario not run is one whose planner never
/// answered with today's form (or no chat turn at all).
// decision: ORCH-735
#[test]
fn the_expected_differences_name_scenarios_that_run() {
    let (all, skips) = eligible();
    let names: Vec<String> = all.iter().map(|r| s(&r.sc["name"])).collect();
    for (area, scs, why) in EXPECTED {
        assert!(!why.is_empty(), "{area}: no reason");
        for sc in *scs {
            assert!(names.iter().any(|n| n == sc), "{area}: {sc} isn't run");
        }
    }
    assert_eq!(
        skips
            .iter()
            .map(|l| l.split(':').next().unwrap())
            .collect::<Vec<_>>(),
        [
            "second_message_merged",
            "last_call_empty",
            "front_window",
            "preview_attention"
        ],
        "{skips:#?}"
    );
}

macro_rules! batches {
    ($($name:ident = $k:expr),*) => {$(
        // decision: ORCH-735
        // decision: ORCH-736
        // decision: ORCH-737
        #[test]
        fn $name() {
            batch($k);
        }
    )*};
}

batches!(
    front_door_replay_1 = 0,
    front_door_replay_2 = 1,
    front_door_replay_3 = 2,
    front_door_replay_4 = 3,
    front_door_replay_5 = 4,
    front_door_replay_6 = 5,
    front_door_replay_7 = 6,
    front_door_replay_8 = 7
);

/// Where two texts first differ, with some of each around it.
fn first_diff(a: &str, b: &str) -> String {
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let at = (0..ac.len().min(bc.len()))
        .find(|i| ac[*i] != bc[*i])
        .unwrap_or(ac.len().min(bc.len()));
    let from = at.saturating_sub(120);
    let show = |c: &[char]| {
        c[from.min(c.len())..(at + 200).min(c.len())]
            .iter()
            .collect::<String>()
    };
    format!(
        "at char {at}\n    planner:    …{}\n    front door: …{}",
        show(&ac),
        show(&bc)
    )
}

fn cut(t: &str) -> String {
    if t.chars().count() > 600 {
        t.chars().take(600).collect::<String>() + "…"
    } else {
        t.to_string()
    }
}

/// ORCH-736: each front tool's arguments as the gateway lists them are what the supervisor's `front_call` takes
/// (`arguments_of`), so a call the gateway admits is never refused for an argument it was offered.
// decision: ORCH-736
#[test]
fn each_front_tool_lists_what_the_supervisor_takes() {
    for name in ferb_orchestrator::lookups::FRONT_TOOLS {
        let a = ferb_gateway::catalog::additions()
            .iter()
            .find(|a| a.name == name)
            .unwrap();
        let listed: Vec<&str> = a.row["input_schema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "why")
            .collect();
        let takes = ferb_supervisor::flow::turn::front_door::arguments_of(name).unwrap();
        assert_eq!(listed, takes, "{name}");
    }
}
