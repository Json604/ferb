//! The 28 chat scenarios recorded from the real Python daemon (engine/fixtures/daemon/chat_scenarios.json), replayed end
//! to end through the real `ferbd daemon` program with a scripted model (the test build's `FERBD_STAND_INS` `script`,
//! src/standins_chat.rs): the recorded request lines are sent over the socket, and compared are every line the app
//! received (bytes and order, per connection), every model call FERB made (runtime role, mode, tools, answer schema,
//! instructions, parts), every reply the scripted model's tool calls got through the socket (as the tool bridge makes
//! them, with the run's token), and the store's tables at the end. A restart stops the process (SIGTERM) and starts
//! the program again on the same home.
//!
//! What a real daemon computes but the recorder answered with stand-ins comes from the recording through the same
//! test feature: the environment, live facts and every role's facts (engine/fixtures/supervisor/scenarios.json), the
//! relevant activity, the desktop observer's context and its spans, the running apps, the file facts the recorder
//! invented, the questions' and new chat's counter ids, the untrusted wrapper's id, the byte-ceiling tokenizer.
//!
//! `CHAT_REPLAY=<name>` runs one scenario; `CHAT_REPLAY_DEBUG=1` prints every difference.

mod support;
// stage 2: the order rules every conversation's stored entries keep (shared with ferb-supervisor's chat_entries)
#[path = "../../ferb-supervisor/tests/support_entries/mod.rs"]
mod entry_rules;
// the deliberate prompt changes since the recording (ENG-6, engine/fixtures/catalog/prompt_changes.json)
#[path = "../../ferb-supervisor/tests/common/prompt_changes.rs"]
mod prompt_changes;
// the deliberate differences RUN-465 and ORCH-650 (tests/common/named_diffs.rs of ferb-supervisor)
#[path = "../../ferb-supervisor/tests/common/named_diffs.rs"]
mod named_diffs;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use support::*;

// ---------------------------------------------------------------------------------------------------------------
// the fixtures
// ---------------------------------------------------------------------------------------------------------------

/// Every context budget line in a text: (reserved_tokens, content_tokens_estimate), the line raw or JSON-escaped.
fn budget_lines(text: &str) -> Vec<(i64, i64)> {
    let number = |key: &str, from: &str| -> Option<(i64, usize)> {
        let at = from.find(key)? + key.len();
        let rest = from[at..].trim_start_matches(['\\', '"', ':', ' ']);
        let n = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        Some((rest[..n].parse().ok()?, from.len() - rest.len() + n))
    };
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((r, end)) = number("reserved_tokens", rest) {
        rest = &rest[end..];
        if let Some((c, _)) = number("content_tokens_estimate", rest) {
            out.push((r, c));
        }
    }
    out
}

/// A budget line `from` (reserved_tokens, content_tokens_estimate) moved `to`, raw or JSON-escaped once or twice.
fn moved_budget(text: &str, from: (i64, i64), to: (i64, i64)) -> String {
    let mut out = text.to_string();
    for q in ["\"", "\\\"", "\\\\\\\""] {
        let line = |(r, c): (i64, i64)| {
            format!("{q}reserved_tokens{q}: {r}, {q}content_tokens_estimate{q}: {c}")
        };
        out = out.replace(&line(from), &line(to));
    }
    out
}

/// A recorded table cell with the Rust engine's own tools named and each model call's budget line moved as that
/// call's was (decision: DMN-595), found by the line's own numbers (the calls are compared first).
fn with_tool_shifts(v: &Value, shifts: &BTreeMap<(i64, i64), (i64, i64)>) -> Value {
    let Value::String(text) = v else {
        return v.clone();
    };
    let mut out = prompt_changes::with_tool_names(text).0;
    for (from, to) in shifts {
        out = moved_budget(&out, *from, *to);
    }
    json!(out)
}

/// A recorded call's tool names with the Rust engine's own tools added (the one named difference; decision: DMN-595):
/// each addition right after the tool it follows wherever that one was offered; the planner's lookups (`sorted`) are
/// listed in name order.
fn with_additions(tools: &Value, sorted: bool) -> Value {
    let Some(list) = tools.as_array() else {
        return tools.clone();
    };
    let mut names: Vec<String> = list
        .iter()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect();
    if names.len() != list.len() {
        return tools.clone();
    }
    for a in ferb_gateway::catalog::additions() {
        // ORCH-663: one offered only by name (task.trace) is in the planner's lookups alone
        if !sorted && a.row["facts"]["named_only"].as_bool().unwrap_or(false) {
            continue;
        }
        if let Some(i) = names.iter().position(|n| *n == a.after) {
            names.insert(i + 1, a.name.clone());
        }
    }
    if sorted {
        names.sort();
    }
    json!(names)
}

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

/// make.py `scrub`: the volatile keys at any depth, in JSON text too.
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

/// 64-hex hashes FERB takes over real paths, compared as hashes only (this run's folder isn't the recorder's).
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
            // the regex `[0-9a-f]{64}` replaces each whole 64 in turn
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

/// The deliberate differences from Python (ENG-6), applied to the recording: (scenario, recorded, the port's).
const FIXES: [(&str, &str, &str); 11] = [
    (
        "global_stop",
        "(from the task) FERB was stopped",
        "FERB was stopped",
    ),
    // GATE-581's kind: the forget question FERB wrote whole is not tagged as the task's (ferb-supervisor's FIXES)
    (
        "remember_then_forget",
        "(from the task) FERB deletes:",
        "FERB deletes:",
    ),
    // the same forget question, and the planner's history of it in the later turns, whose context is then 16 bytes
    // shorter: their budget line counts 16 tokens fewer (ferb-supervisor's FIXES)
    (
        "displaced_question",
        "(from the task) FERB deletes:",
        "FERB deletes:",
    ),
    ("displaced_question", "\": 2715,", "\": 2699,"),
    ("displaced_question", "\": 3422,", "\": 3406,"),
    ("displaced_question", "\": 4422,", "\": 4406,"),
    // ORCH-642 (fixed against Python): the repair also names the dependency the plan lacks, raw, JSON-escaped once and
    // twice, and the repair call's budget line grows by those 42 bytes (ferb-supervisor's FIXES)
    (
        "plan_repair",
        "answer_step.\"]",
        "answer_step.\", \"write: missing dependencies ['gather']\"]",
    ),
    (
        "plan_repair",
        "answer_step.\\\"]",
        "answer_step.\\\", \\\"write: missing dependencies ['gather']\\\"]",
    ),
    (
        "plan_repair",
        "answer_step.\\\\\\\"]",
        "answer_step.\\\\\\\", \\\\\\\"write: missing dependencies ['gather']\\\\\\\"]",
    ),
    ("plan_repair", "\": 2139,", "\": 2181,"),
    // ORCH-649 (fixed against Python): a conversation change from chat is said by who the chat is with, not as
    // "Conversation <action>: <id>" (ferb-supervisor's FIXES)
    (
        "conversation_change",
        "Conversation pause: id-000021:chat",
        "Paused the chat with Robin.",
    ),
];

/// ORCH-647 (fixed against Python): in these scenarios the task's only live step is a wait, so the port also tells
/// the wait's own line (a `progress` frame whose step task is the task itself, "Waiting …") that Python never sent;
/// those frames are left out before the frames are compared, and only those.
const WAIT_LINES: [&str; 2] = ["event_wait", "wait_step"];

/// ORCH-648 (fixed against Python): in these scenarios the planner gives the owner's identity to a task naming sites,
/// so the port also asks the browser area where it browses (`browsing`), which Python asked only for identity "ferb";
/// the recording has no answer, so it is `{}` (nothing picked, nothing asked: the task runs as Python ran it).
const OWNER_BROWSING: [&str; 1] = ["login_sites"];

/// A frame ORCH-647 added: a task-level `progress` frame with a wait's line.
fn wait_line_frame(scenario: &str, line: &str) -> bool {
    if !WAIT_LINES.contains(&scenario) {
        return false;
    }
    let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
    let p = &v["params"];
    p["type"] == "progress"
        && p["step_task_id"] == p["task_id"]
        && p["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("Waiting "))
}

fn fixed(scenario: &str, text: &str) -> String {
    FIXES
        .iter()
        .filter(|(n, _, _)| *n == scenario)
        .fold(text.to_string(), |t, (_, a, b)| t.replace(a, b))
}

fn line_of(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        other => s(&resolve(other)),
    }
}

/// How long the recorder's temporary root was: the budget line counts its bytes.
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

/// The run's folder: as long as the recorder's, under the system's temp folder (spelled as it was), never its own.
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

// ---------------------------------------------------------------------------------------------------------------
// one scenario
// ---------------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    frames: Vec<String>,
    calls: Vec<String>,
    tables: Vec<String>,
    /// stage 2: the chat_entries table against the golden entries, and the order rules
    entries: Vec<String>,
    /// stage 2: the engine's own interleaving as a timeline, with its stored entries (`chat-entries/engine_*.json`)
    engine_order: Option<Value>,
    stopped: Option<String>,
    checked: usize,
}

impl Report {
    fn full(&self) -> bool {
        self.stopped.is_none()
            && self.frames.is_empty()
            && self.calls.is_empty()
            && self.tables.is_empty()
            && self.entries.is_empty()
    }
}

const NOT_COMPARED: [&str; 3] = ["perception_floor", "recall_state", "trail_changes"];

/// make.py `tables`: the recorder leaves out every full-text index (`_fts` in the name) with its shadow tables. The
/// stage-2 tables (migration 0063: the chat's entries, ENG-8) did not exist when Python recorded: they are compared
/// with the chat-entries golden instead ([`entries_against_golden`]).
fn compared(table: &str) -> bool {
    !NOT_COMPARED.contains(&table)
        && !ferb_store::chat_entries::TABLES.contains(&table)
        && !table.starts_with("learning_")
        && !table.contains("_fts")
}

// ---------------------------------------------------------------------------------------------------------------
// stage 2: the wired projector against the pure one
// ---------------------------------------------------------------------------------------------------------------

/// The golden entries of every recorded scenario (`engine/fixtures/chat-entries/scenarios.json`, ferb-supervisor's
/// `ChatProjector` folded over each scenario's timeline), by name.
fn golden_entries() -> &'static HashMap<String, Vec<Value>> {
    static G: OnceLock<HashMap<String, Vec<Value>>> = OnceLock::new();
    G.get_or_init(|| {
        fixture("chat-entries/scenarios.json")["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sc| (s(&sc["name"]), sc["entries"].as_array().unwrap().clone()))
            .collect()
    })
}

/// Entries as compared: per conversation in `pos` order; ids, the entries they name (`answers`, a later answer's
/// `follows`) and `pos` replaced by their rank (the live engine's ids are its own, its positions start at
/// `POS_BASE`); everything else as stored.
fn normalized(entries: &[Value]) -> Vec<Value> {
    let mut list: Vec<Value> = entries.to_vec();
    list.sort_by(|a, b| {
        (s(&a["conversation_id"]), a["pos"].as_u64())
            .cmp(&(s(&b["conversation_id"]), b["pos"].as_u64()))
    });
    let names: HashMap<String, String> = list
        .iter()
        .enumerate()
        .map(|(i, e)| (s(&e["id"]), format!("e{}", i + 1)))
        .collect();
    let rename = |v: &Value| match v.as_str() {
        Some(id) => json!(names
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("unknown:{id}"))),
        None => v.clone(),
    };
    let mut rank: HashMap<String, u64> = HashMap::new();
    list.iter()
        .map(|e| {
            let mut e = e.clone();
            e["id"] = rename(&e["id"]);
            e["answers"] = rename(&e["answers"]);
            if e["body"].get("follows").is_some() {
                e["body"]["follows"] = rename(&e["body"]["follows"]);
            }
            let r = rank.entry(s(&e["conversation_id"])).or_default();
            *r += 1;
            e["pos"] = json!(*r);
            e
        })
        .collect()
}

/// Where the golden's timeline and what the engine saw differ, by scenario (the golden is the pure projector over a
/// timeline built from the recording, and that timeline is a choice: a recording keeps each connection's lines in
/// order, not their order across connections, so it places a stream's frames where the stream ended, takes a question
/// no stream carried from `chat.status`, and dates what came later with the last time it saw; the engine takes each
/// thing when it happens). Every other scenario is compared strictly.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Diff {
    /// Two streams were open at once, or a question came beside a stream: the engine's interleaving is the real one,
    /// the golden's puts the stream's frames at its end. Compared: the same entries, each in its final state, whatever
    /// their order (`pos`, `seq`, `rev` and the ids left out; the entries an answer names compared by kind and request).
    Order,
    /// A question the golden took from `chat.status` (no request, its task the tree) and the engine from the event
    /// itself (its request, its worker's task): `request_id` and `task_id` of questions left out.
    StatusQuestion,
    /// The clock moved during the scenario (a reminder, a schedule's run, a pause across a day): what was made or
    /// revised after it carries the engine's time, the golden the last time its timeline saw. Times left out.
    Times,
    /// A question no stream carried and no `chat.status` of the recording listed (the recording ended before it was
    /// polled): the engine posts it as it is asked; the golden never saw it. Questions the golden lacks left out.
    UnlistedQuestion,
    /// A task's result the engine took from the bus (`task.result`) that no stream carried and no `chat.status` of
    /// the recording listed (it had been shown): the golden never saw it. Answers with words the golden lacks left out.
    UnlistedResult,
    /// A restart in the middle: the engine's start-up pass expired the question left open (pending questions die with
    /// the process), which the golden's timeline doesn't model; the golden's open questions are expired here.
    Restart,
}

const ENTRY_DIFFERENCES: &[(&str, &[Diff])] = &[
    ("confirm_yes", &[Diff::Order, Diff::StatusQuestion]),
    ("confirm_no", &[Diff::Order, Diff::StatusQuestion]),
    ("pause_steer_resume", &[Diff::Order]),
    ("steer_running_task", &[Diff::Order]),
    ("stop_by_chat", &[Diff::Order]),
    ("stop_plus_new_task", &[Diff::Order]),
    ("confirm_stop_pending", &[Diff::Order, Diff::StatusQuestion]),
    (
        "worker_question_answered",
        &[Diff::Order, Diff::StatusQuestion],
    ),
    ("check_in_after_five", &[Diff::Order, Diff::StatusQuestion]),
    (
        "restart_with_open_question",
        &[Diff::Order, Diff::StatusQuestion, Diff::Restart],
    ),
    ("schedule_created_and_fired", &[Diff::Times]),
    ("timer_set_and_fired", &[Diff::Times]),
    // the newer recordings (supervisor/more_scenarios.json)
    ("answer_in_chat", &[Diff::Order, Diff::StatusQuestion]),
    ("ask_step", &[Diff::Order, Diff::StatusQuestion]),
    ("route_question", &[Diff::Order, Diff::StatusQuestion]),
    ("checker_runtime_down", &[Diff::Order, Diff::StatusQuestion]),
    (
        "correct_pending_check",
        &[Diff::Order, Diff::StatusQuestion],
    ),
    ("loop_pause_stop", &[Diff::Order, Diff::StatusQuestion]),
    ("owner_closed_tab", &[Diff::Order, Diff::StatusQuestion]),
    (
        "conversation_handover",
        &[Diff::Order, Diff::StatusQuestion],
    ),
    ("conversation_change", &[Diff::Order, Diff::StatusQuestion]),
    (
        "conversation_resume",
        &[Diff::Order, Diff::StatusQuestion, Diff::UnlistedResult],
    ),
    ("loop_steer_finish", &[Diff::Order]),
    (
        "mention_paused",
        &[Diff::Order, Diff::StatusQuestion, Diff::Times],
    ),
    ("stated_scope", &[Diff::Order]),
    ("browse_as_corrected", &[Diff::Order]),
    (
        "remember_then_forget",
        &[Diff::Order, Diff::UnlistedQuestion],
    ),
    ("displaced_question", &[Diff::Order, Diff::UnlistedQuestion]),
];

/// The entries with a scenario's named differences applied (`golden`: the golden's side).
fn with_differences(entries: Vec<Value>, diffs: &[Diff], golden: bool) -> Vec<Value> {
    let by_id: HashMap<String, Value> = entries.iter().map(|e| (s(&e["id"]), e.clone())).collect();
    let mut out: Vec<Value> = entries
        .into_iter()
        .map(|mut e| {
            let kind = s(&e["kind"]);
            if diffs.contains(&Diff::Restart) && golden && kind == "question" && e["open"] == true {
                e["state"] = json!("expired");
                e["open"] = json!(false);
                e["body"]["answer"] =
                    json!({"state": "expired", "how": null, "choice": null, "said": null});
            }
            if diffs.contains(&Diff::StatusQuestion) && kind == "question" {
                e["request_id"] = Value::Null;
                e["task_id"] = Value::Null;
            }
            if diffs.contains(&Diff::Times) {
                for k in ["created_at", "updated_at", "day"] {
                    e[k] = Value::Null;
                }
            }
            e
        })
        .collect();
    if diffs.contains(&Diff::Order) || diffs.contains(&Diff::Restart) {
        let named = |id: &Value| match id.as_str().and_then(|i| by_id.get(i)) {
            Some(r) => json!(format!("{}:{}", s(&r["kind"]), s(&r["request_id"]))),
            None => id.clone(),
        };
        for e in &mut out {
            e["seq"] = Value::Null;
            e["rev"] = Value::Null;
            if diffs.contains(&Diff::Order) {
                e["answers"] = named(&e["answers"]);
                if e["body"].get("follows").is_some() {
                    e["body"]["follows"] = named(&e["body"]["follows"]);
                }
                e["pos"] = Value::Null;
            }
        }
        if diffs.contains(&Diff::Order) {
            for e in &mut out {
                e["id"] = Value::Null;
            }
            out.sort_by_key(|e| e.to_string());
        }
    }
    out
}

/// After a scenario, the `chat_entries` table equals the golden entries (the pure projector over the recorded
/// timeline), normalized, with the scenario's named differences: the wired projector made the same conversation. A
/// difference is reported per entry.
fn entries_against_golden(name: &str, db: &Path) -> Vec<String> {
    let Some(want) = golden_entries().get(name) else {
        return vec![format!("{name}: no golden entries")];
    };
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut mine = Vec::new();
    for c in ferb_store::chat_entries::conversations(&conn).unwrap() {
        let page = ferb_store::chat_entries::page(&conn, &c, None, None, 100_000).unwrap();
        mine.extend(
            page.entries
                .iter()
                .map(|e| serde_json::to_value(e).unwrap()),
        );
    }
    let diffs: &[Diff] = ENTRY_DIFFERENCES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| *d)
        .unwrap_or(&[]);
    if diffs.contains(&Diff::UnlistedQuestion) {
        let asked: BTreeSet<String> = want
            .iter()
            .filter(|e| e["kind"] == "question")
            .map(|e| s(&e["approval_id"]))
            .collect();
        mine.retain(|e| e["kind"] != "question" || asked.contains(&s(&e["approval_id"])));
    }
    if diffs.contains(&Diff::UnlistedResult) {
        let said: BTreeSet<String> = want
            .iter()
            .filter(|e| e["kind"] == "answer")
            .map(|e| s(&e["body"]["text"]))
            .collect();
        mine.retain(|e| e["kind"] != "answer" || said.contains(&s(&e["body"]["text"])));
    }
    let (want, mine) = (
        with_differences(normalized(want), diffs, true),
        with_differences(normalized(&mine), diffs, false),
    );
    let mut out = Vec::new();
    for i in 0..want.len().max(mine.len()) {
        match (want.get(i), mine.get(i)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                let keys: Vec<String> = a
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(k, v)| b.get(k.as_str()) != Some(v))
                    .map(|(k, v)| {
                        format!(
                            "{k}: golden {} / table {}",
                            cut(&v.to_string()),
                            cut(&b[k].to_string())
                        )
                    })
                    .collect();
                out.push(format!(
                    "entry {} ({}): {}",
                    i + 1,
                    s(&a["kind"]),
                    keys.join("; ")
                ));
            }
            (a, b) => out.push(format!(
                "entry {}: golden {} / table {}",
                i + 1,
                a.map(|v| cut(&v.to_string())).unwrap_or("(none)".into()),
                b.map(|v| cut(&v.to_string())).unwrap_or("(none)".into())
            )),
        }
    }
    out
}

/// Stage 2's order check (journal 72's open item: for the scenarios compared as "the same entries whatever their order",
/// the stored order was checked against nothing). After every scenario: the rules of `support_entries` over the chat
/// log's trace (every input the projectors were fed, in the order the engine fed them) and the stored entries; the
/// store's own `changes` after every cut equal to the stored entries above it, in `seq` order; and the engine's real
/// interleaving written as a timeline FERB.app's reducer is driven with (`rust/fixtures/chat-entries/engine_*.json`,
/// EntryFoldTests `testTheEnginesOwnInterleavingFoldsToWhatTheAppShows`).
fn entry_order(
    name: &str,
    db: &Path,
    trace_path: &Path,
    run_root: &RunRoot,
) -> (Vec<String>, Value) {
    use ferb_core::chat_entry::Entry;
    let items: Vec<Value> = std::fs::read_to_string(trace_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(&run_root.mask(l)).ok())
        .collect();
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mask = |e: &Entry| -> Entry {
        serde_json::from_str(&run_root.mask(&serde_json::to_string(e).unwrap())).unwrap()
    };
    let conversations = ferb_store::chat_entries::conversations(&conn).unwrap();
    let mut stored: Vec<Entry> = Vec::new();
    let mut out = Vec::new();
    for c in &conversations {
        let page = ferb_store::chat_entries::page(&conn, c, None, None, 100_000).unwrap();
        let mine: Vec<Entry> = page.entries.iter().map(mask).collect();
        // what a client catching up reads: the store's changes after every cut, in seq order
        let mut by_seq = mine.clone();
        by_seq.sort_by_key(|e| e.seq);
        let mut cuts = vec![0];
        cuts.extend(by_seq.iter().map(|e| e.seq));
        for k in cuts {
            let got: Vec<Entry> = ferb_store::chat_entries::changes(&conn, c, k, 100_000)
                .unwrap()
                .entries
                .iter()
                .map(mask)
                .collect();
            let want: Vec<&Entry> = by_seq.iter().filter(|e| e.seq > k).collect();
            if got.iter().collect::<Vec<_>>() != want {
                out.push(format!(
                    "{name} [{c}]: changes after {k} give {:?}, the entries above it are {:?} (rule 7, the store)",
                    got.iter().map(|e| (&e.id, e.seq)).collect::<Vec<_>>(),
                    want.iter().map(|e| (&e.id, e.seq)).collect::<Vec<_>>()
                ));
                break;
            }
        }
        stored.extend(mine);
    }
    let trace = entry_rules::read(&items);
    out.extend(entry_rules::check(name, &trace, &stored));
    // the engine's interleaving as a timeline (the words of `chat-entries/scenarios.json`): each input the projectors
    // were fed, in that order; a held bus copy where it was fed (the stream's end), a bus copy the stream fed itself
    // left out; every bus input as the app's own `logs.subscribe` gives it (`bus`)
    let mut timeline = Vec::new();
    let mut streams: HashMap<String, usize> = HashMap::new();
    for item in &items {
        let Some(session) = item.get("conversation").and_then(Value::as_str) else {
            continue;
        };
        let input = &item["input"];
        let request = s(&input["request_id"]);
        match input["do"].as_str().unwrap_or("") {
            "frame" => {
                let mut frame = input["frame"].clone();
                if frame["type"] == "ack" && !streams.contains_key(&request) {
                    let n = streams.len();
                    streams.insert(request.clone(), n);
                    let paths = frame
                        .get("input_paths")
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    timeline.push(json!({"do": "send", "stream": n, "session": session,
                        "request_id": request, "text": frame["message"], "input_paths": paths,
                        "at": frame["at"]}));
                }
                // what the engine added to the ack it fed (the stream's own frame carries none of it)
                if let Some(f) = frame.as_object_mut() {
                    for k in ["input_paths", "client_id", "submission_id"] {
                        f.remove(k);
                    }
                }
                let stream = streams.get(&request).copied();
                timeline.push(
                    json!({"do": "frame", "stream": stream, "session": session, "frame": frame}),
                );
            }
            "end" => {
                let stream = streams.get(&request).copied();
                timeline.push(json!({"do": "end", "stream": stream, "session": session}));
            }
            "bus" => timeline.push(json!({"do": "bus", "session": session, "topic": input["topic"],
                "data": input["data"], "at": input["data"].get("at").cloned().unwrap_or(Value::Null)})),
            "notification" => timeline
                .push(json!({"do": "notification", "session": session, "item": input["item"]})),
            "restart" => timeline.push(json!({"do": "restart", "session": session})),
            _ => {}
        }
    }
    // the log's entry ids are random (its id source is never the daemon's): named by their place, as the golden's
    let names: HashMap<String, String> = stored
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.clone(), format!("ce-{:06}", i + 1)))
        .collect();
    let rename = |v: &Value| match v.as_str() {
        Some(id) => json!(names
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("unknown:{id}"))),
        None => v.clone(),
    };
    let entries: Vec<Value> = stored
        .iter()
        .map(|e| {
            let mut v = serde_json::to_value(e).unwrap();
            v["id"] = rename(&v["id"]);
            v["answers"] = rename(&v["answers"]);
            if v["body"].get("follows").is_some() {
                v["body"]["follows"] = rename(&v["body"]["follows"]);
            }
            v
        })
        .collect();
    let export = json!({"name": name, "timeline": timeline, "entries": entries});
    (out, export)
}

fn is_shadow(name: &str) -> bool {
    ["_data", "_idx", "_content", "_docsize", "_config"]
        .iter()
        .any(|x| name.ends_with(x))
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

fn dump(db: &Path, run: &RunRoot) -> BTreeMap<String, (Vec<String>, Vec<Vec<Value>>)> {
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
        if rows.is_empty() {
            continue;
        }
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
        "at char {at}\n    python: …{}\n    rust:   …{}",
        show(&ac),
        show(&bc)
    )
}

fn cut(t: &str) -> String {
    if t.chars().count() > 900 {
        t.chars().take(900).collect::<String>() + "…"
    } else {
        t.to_string()
    }
}

/// The script file for one process of a scenario: the recorded calls, what the supervisor read, the desktop
/// observer's rows, the run's folder, the owner's file.
struct Script {
    calls: Vec<Value>,
    gated: BTreeSet<usize>,
    supervise: Vec<Value>,
    systems: Vec<Value>,
    contexts: Vec<Value>,
    availability: String,
    spans: Value,
    note: Value,
    /// The front windows the recorder's daemon captured at send (`captures`), by request.
    captures: Value,
    /// The memory and browser ports' recorded answers (`port` records), in order.
    ports: Vec<Value>,
}

fn catalogs() -> &'static Value {
    static C: OnceLock<Value> = OnceLock::new();
    C.get_or_init(|| resolve(&fixture("supervisor/scenarios.json")["catalogs"]))
}

fn records() -> &'static Map<String, Value> {
    static R: OnceLock<Map<String, Value>> = OnceLock::new();
    R.get_or_init(|| {
        fixture("supervisor/scenarios.json")["scenarios"]
            .as_object()
            .unwrap()
            .clone()
    })
}

/// A base scenario's records (engine/fixtures/supervisor/scenarios.json), bodies resolved.
fn base_records(name: &str) -> Vec<Value> {
    records()[name]["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(resolve)
        .collect()
}

fn script_of(sc: &Value, recs: &[Value], catalogs: &Value, workspace: &str) -> Script {
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
    let calls: Vec<Value> = sc["model_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(resolve)
        .collect();
    // the calls held until `release_model`: each the call the model wait before it named
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
    // a `note` step's file: "the owner writes workspace/<path> (<its text, JSON-quoted>) …"
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
    }
}

/// "the model has received call 'key' (n)": the index of that call in the recording.
fn model_wait(calls: &[Value], until: &str) -> Option<usize> {
    let rest = until.strip_prefix("the model has received call ")?;
    let (key, n) = rest.rsplit_once(" (")?;
    let key = key.trim_matches('\'');
    let n: usize = n.trim_end_matches(')').parse().ok()?;
    calls
        .iter()
        .enumerate()
        .filter(|(_, m)| m["key"] == key)
        .nth(n.checked_sub(1)?)
        .map(|(i, _)| i)
}

/// What one process of the scenario used of the script (carried into the next after a restart).
#[derive(Default)]
struct Used {
    made: Vec<Value>,
    started: Vec<Value>,
    answered: Vec<usize>,
    tool_diffs: Vec<String>,
    tool_calls: usize,
    supervise: usize,
    systems: usize,
    contexts: usize,
    spans: Vec<Value>,
    problems: Vec<String>,
    /// The questions asked so far (the recorder's one counter per scenario).
    questions: u64,
}

impl Used {
    fn take(&mut self, st: &Value) {
        self.made
            .extend(st["made"].as_array().cloned().unwrap_or_default());
        self.started
            .extend(st["started"].as_array().cloned().unwrap_or_default());
        self.answered.extend(
            st["answered"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|v| v as usize),
        );
        self.tool_diffs.extend(strs(&st["tool_diffs"]));
        self.tool_calls += st["tool_calls"].as_u64().unwrap_or(0) as usize;
        self.supervise += st["supervise_used"].as_u64().unwrap_or(0) as usize;
        self.systems += st["systems_used"].as_u64().unwrap_or(0) as usize;
        self.contexts += st["contexts_used"].as_u64().unwrap_or(0) as usize;
        self.spans = st["spans_used"].as_array().cloned().unwrap_or_default();
        self.problems.extend(strs(&st["problems"]));
        self.questions = st["questions"].as_u64().unwrap_or(self.questions);
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
    });
    std::fs::write(path, file.to_string()).unwrap();
}

fn config(script: &Path, ids: &str) -> Value {
    // the recorder's programs: the pinned Codex installed, never run (b_runtimes.rs `World`)
    json!({"group_a": false, "ids": ids, "script": script.to_string_lossy(), "fs_identity": true,
           "programs": "recorder"})
}

/// Until FERB is quiet: no new journal row or span for a while.
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

/// The journal rows the recorder's daemon wrote at its start, in place of this program's own (the scenario's ids
/// continue from them).
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

fn run_scenario(
    sc: &Value,
    recs: &[Value],
    catalogs: &Value,
    index: usize,
    run_root: &RunRoot,
    schemas: &Map<String, Value>,
) -> Report {
    let mut report = Report::default();
    let name = s(&sc["name"]);
    let nn = format!("{:02}", index + 1);
    let root = PathBuf::from(&run_root.root);
    let (home, user) = Run::paths(&root, &nn);
    // two recordings made in separate runs can share a folder number: each scenario starts on an empty home (the
    // one before it has stopped)
    let _ = std::fs::remove_dir_all(root.join(&nn));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let script = script_of(sc, recs, catalogs, &workspace.to_string_lossy());
    let script_path = root.join(format!("{nn}-script.json"));
    let mut used = Used::default();
    write_script(&script_path, &script, &used, run_root);
    // stage 2: the chat log's trace (every input the projectors were fed, in order: the order check)
    let trace_path = root.join(format!("{nn}-trace.jsonl"));
    let _ = std::fs::remove_file(&trace_path);
    let mut run = Run::start(&root, &nn, {
        let mut c = config(&script_path, "id");
        c["chat_trace"] = json!(trace_path.to_string_lossy());
        c
    });
    // the recorder's start: its journal rows, then the scenario's ids; the timer claim tokens where its loop was
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
    // a stream's request id, by connection
    let mut streams: BTreeMap<String, Value> = BTreeMap::new();
    let mut closed: BTreeMap<String, Raw> = BTreeMap::new();
    let want_line = |l: &Value| fixed(&name, &line_of(l));
    for st in sc["steps"].as_array().unwrap() {
        let conn = s(&st["conn"]);
        match st["do"].as_str().unwrap() {
            "connect" => {
                let c = Raw::connect(&run.socket()).unwrap();
                conns.insert(conn, c);
            }
            // the recorder's seeded journal rows are the start's rows here (`recorded_start`)
            "seed" => {}
            // an invented file the owner writes (make_more.py `write_file`), in this run's home
            "write_file" => {
                let path = home.join(s(&st["path"]));
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).unwrap();
                }
                std::fs::write(&path, s(&st["text"])).unwrap();
            }
            "close" => {
                // a stream it held may still be read (the recorder closed, then took what had arrived: `stream_cut`)
                if let Some(c) = conns.remove(&conn) {
                    if streams.contains_key(&conn) {
                        closed.insert(conn, c);
                    }
                }
            }
            "call" => {
                let c = conns.get_mut(&conn).expect("an open connection");
                for l in st["lines"].as_array().unwrap() {
                    if let Some(sent) = l.get("send") {
                        c.send(&line_of(sent));
                    } else if let Some(r) = l.get("recv") {
                        let want = want_line(r);
                        let got = c
                            .recv()
                            .map(|g| run_root.mask(&g))
                            .unwrap_or("(closed)".into());
                        report.checked += 1;
                        if got != want {
                            report.frames.push(format!(
                                "{conn}\n    python: {}\n    rust:   {}",
                                cut(&want),
                                cut(&got)
                            ));
                        }
                    }
                }
                settle(&run);
            }
            "stream" => {
                let sent = line_of(&st["lines"][0]["send"]);
                let v: Value = serde_json::from_str(&sent).unwrap();
                streams.insert(conn.clone(), v["id"].clone());
                conns
                    .get_mut(&conn)
                    .expect("an open connection")
                    .send(&sent);
            }
            "stream_end" | "stream_end_at_shutdown" | "stream_cut" => {
                let want: Vec<String> = st["lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|l| l.get("recv"))
                    .map(want_line)
                    .collect();
                let Some(c) = conns.get_mut(&conn).or_else(|| closed.get_mut(&conn)) else {
                    report.stopped = Some(format!("{conn}'s stream, never opened here"));
                    break;
                };
                let mut got = Vec::new();
                if st["do"] == "stream_cut" {
                    for _ in 0..want.len() {
                        match c.recv() {
                            Some(l) => got.push(run_root.mask(&l)),
                            None => break,
                        }
                    }
                    conns.remove(&conn);
                    closed.remove(&conn);
                } else {
                    let id = streams.get(&conn).cloned().unwrap_or(Value::Null);
                    while let Some(l) = c.recv() {
                        let v: Value = serde_json::from_str(&l).unwrap_or(Value::Null);
                        if wait_line_frame(&name, &l) {
                            continue;
                        }
                        got.push(run_root.mask(&l));
                        if v["id"] == id {
                            break;
                        }
                    }
                }
                report.checked += want.len();
                for i in 0..want.len().max(got.len()) {
                    let (w, g) = (want.get(i), got.get(i));
                    if w != g {
                        report.frames.push(format!(
                            "{conn} line {i}\n    python: {}\n    rust:   {}",
                            cut(w.map(String::as_str).unwrap_or("(none)")),
                            cut(g.map(String::as_str).unwrap_or("(none)"))
                        ));
                        break;
                    }
                }
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
                    // make_more.py `probe_reads`: the loop's armed probe has read through the gateway (its baseline,
                    // then the watch's first read, at once). An unchanged read writes no record, so the program is
                    // given a moment once the loop watches (the read is a local file's, well under it)
                    std::thread::sleep(std::time::Duration::from_millis(500));
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
                    report.stopped = Some(format!("a wait for {until} (never true here)"));
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
            // make_more.py: `Supervisor.preview` on the daemon's loop, its events collected
            "preview" => {
                let got = run.control(json!({"do": "preview", "session_id": st["session_id"],
                                             "text": st["text"]}));
                let got: Value = serde_json::from_str(&run_root.mask(&got.to_string())).unwrap();
                report.checked += 1;
                let want = resolve(&st["events"]);
                if got != want {
                    report
                        .frames
                        .push(format!("preview\n    python: {want}\n    rust:   {got}"));
                }
                settle(&run);
            }
            // make_more.py: the attention check (`Supervisor.attention`)
            "attention" => {
                let got = run.control(json!({"do": "attention", "source_refs": st["source_refs"],
                                             "observations": st["observations"],
                                             "already_told": st["already_told"]}));
                report.checked += 1;
                let want = resolve(&st["result"]);
                if got != want {
                    report
                        .frames
                        .push(format!("attention\n    python: {want}\n    rust:   {got}"));
                }
                settle(&run);
            }
            "plan_event" => {
                run.control(json!({"do": "plan_event", "topic": st["topic"], "data": st["data"]}));
                settle(&run);
            }
            "runtime_switch" => {
                let got = run.control(json!({"do": "runtime_switch", "task_id": st["task_id"],
                                             "runtime": st["runtime"]}));
                report.checked += 2;
                if got["offer"] != st["offer"] {
                    report.frames.push(format!(
                        "runtime_offer {}, recorded {}",
                        got["offer"], st["offer"]
                    ));
                }
                if got["answered"] != st["answered"] {
                    report.frames.push(format!(
                        "answer_runtime_switch {}, recorded {}",
                        got["answered"], st["answered"]
                    ));
                }
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
                report.checked += 1;
                if fired != st["fired"] {
                    report.frames.push(format!(
                        "timers_tick fired {fired}, recorded {}",
                        st["fired"]
                    ));
                }
                settle(&run);
            }
            "restart" => {
                // the process really stops (SIGTERM: main.py's clean shutdown) and starts again on the same home
                let (status, err) = run.stop();
                if !status.success() {
                    report.stopped = Some(format!("the daemon's shutdown: {status} {err}"));
                    break;
                }
                // what the stopped process used, its shutdown included
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
                        c["chat_trace"] = json!(trace_path.to_string_lossy());
                        c
                    }
                    .to_string(),
                )
                .unwrap();
                run.spawn();
                settle(&run);
            }
            other => {
                report.stopped = Some(format!("the step {other:?}"));
                break;
            }
        }
    }
    settle(&run);
    used.take(&run.control(json!({"do": "script"})));
    // everything the chat log has queued, written (its writer drains a moment after each wake)
    run.control(json!({"do": "chat_flush"}));
    // the store as the recorder read it: before the daemon's shutdown
    let mine = dump(&home.join("ferb.db"), run_root);
    report.entries = entries_against_golden(&name, &home.join("ferb.db"));
    let (rules, export) = entry_order(&name, &home.join("ferb.db"), &trace_path, run_root);
    report.entries.extend(rules);
    report.engine_order = Some(export);
    if std::env::var("CHAT_REPLAY_DUMP").is_ok() {
        for t in ["journal", "spans"] {
            let at = if t == "journal" { 1 } else { 0 };
            let kind = 3;
            let ours: Vec<String> = mine
                .get(t)
                .map(|(_, r)| r.iter().map(|r| format!("{} {}", r[at], r[kind])).collect())
                .unwrap_or_default();
            let theirs: Vec<String> = sc["tables"][t]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| format!("{} {}", r[at], r[kind]))
                .collect();
            eprintln!("{t}\n  python: {theirs:?}\n  rust:   {ours:?}");
        }
    }
    drop(conns);
    let _ = run.stop();

    // the scripted model's tool calls through the socket, each reply as the Python daemon gave it
    let bridged: usize = script
        .calls
        .iter()
        .flat_map(|m| {
            m["tool_bridge_lines"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|l| {
            l.get("send")
                .and_then(Value::as_str)
                .is_some_and(|t| t.contains("\"tools.call\""))
        })
        .count();
    report.checked += bridged;
    report.calls.extend(used.tool_diffs.iter().cloned());
    if used.tool_calls != bridged {
        report.calls.push(format!(
            "{} tool calls made, {bridged} recorded",
            used.tool_calls
        ));
    }
    report.calls.extend(
        used.problems
            .iter()
            .filter(|p| {
                !(OWNER_BROWSING.contains(&name.as_str())
                    && p.as_str() == "port browsing: not called in the recording")
            })
            .cloned(),
    );
    // the model calls
    let mut tool_shifts: BTreeMap<(i64, i64), (i64, i64)> = BTreeMap::new();
    for (i, want) in script.calls.iter().enumerate() {
        let Some(made) = used.made.get(i) else {
            report
                .calls
                .push(format!("call {} ({}) was not made", i + 1, s(&want["key"])));
            break;
        };
        // the recorder names a schema only from ferb/context/schemas.py; the attention check's (supervisor.py:5509) is
        // the catalog's ATTENTION_SCHEMA, which the recorder names by its hash
        // ORCH-652's gaps, not Python's, are taken out first (the named change)
        let python = ferb_context::schemas::as_recorded(&made["schema"]); // and HAND-580's task profiles
        let schema_name = schemas
            .iter()
            .find(|(k, v)| **v == python && k.as_str() != "ATTENTION_SCHEMA")
            .map(|(k, _)| k.clone())
            .or_else(|| schema_by_sha(&python).filter(|n| n != "ATTENTION_SCHEMA"))
            .unwrap_or_else(|| {
                use sha2::Digest;
                let sha = hex::encode(sha2::Sha256::digest(
                    ferb_py::json::dumps(&python).as_bytes(),
                ));
                format!("sha256:{sha}")
            });
        let started = used.started.iter().find(|c| c["n"] == json!(i));
        // the recorded budget lines reserve for the system as changed since (ENG-6)
        let shift = prompt_changes::reserved_shift(&s(&want["system"]));
        // decision: DMN-595: the Rust engine's own tools offered beside the recorded ones are reserved for too; the
        // span that records this call's context has the same budget line, moved the same (`tool_shifts`)
        let by_tools = prompt_changes::tool_shift(&strs(&want["tools"]));
        let recorded: Vec<(String, i64)> = strs(&want["parts"])
            .iter()
            .map(|p| {
                prompt_changes::with_tool_names(&prompt_changes::shift_reserved(&unhash(p), shift))
            })
            .collect();
        let named: i64 = recorded.iter().map(|(_, n)| n).sum();
        let parts: Vec<String> = recorded
            .iter()
            .map(|(p, _)| {
                let mut p = p.clone();
                for (r, c) in budget_lines(&p) {
                    let to = (r + by_tools, c + named);
                    if to != (r, c) {
                        tool_shifts.insert((r, c), to);
                        p = moved_budget(&p, (r, c), to);
                    }
                }
                fixed(&name, &p)
            })
            .collect();
        let expected = json!({"runtime_role": want["runtime_role"], "mode": want["mode"],
            "tools": with_additions(&want["tools"], want["runtime_role"] == "orchestrator"),
            "schema": want["schema"], "effort": want["effort"], "task_id": want["task_id"], "system": want["system"],
            "parts": parts, "cacheable": want["cacheable"], "images": want["images"]});
        let actual = match started {
            None => {
                json!({"runtime_role": made["runtime_role"], "mode": made["mode"], "tools": made["tools"],
                "schema": schema_name, "effort": made["effort"], "started": false})
            }
            Some(st) => {
                json!({"runtime_role": made["runtime_role"], "mode": made["mode"], "tools": made["tools"],
                "schema": schema_name, "effort": made["effort"], "task_id": st["task_id"], "system": st["system"],
                "parts": strs(&st["parts"]).iter().map(|p| unhash(p)).collect::<Vec<_>>(),
                "cacheable": st["cacheable"], "images": st["images"]})
            }
        };
        report.checked += 1;
        let (expected, actual) = named_diffs::named_call(expected, actual, &mut Vec::new());
        if expected != actual {
            let field = expected
                .as_object()
                .unwrap()
                .keys()
                .find(|k| expected[*k] != actual[*k])
                .cloned()
                .unwrap_or_default();
            let detail = if field == "parts" {
                let (e, a) = (
                    expected["parts"].as_array().unwrap(),
                    actual["parts"].as_array().cloned().unwrap_or_default(),
                );
                let at = (0..e.len().max(a.len()))
                    .find(|i| e.get(*i) != a.get(*i))
                    .unwrap_or(0);
                format!(
                    "part {at} {}",
                    first_diff(
                        &s(e.get(at).unwrap_or(&Value::Null)),
                        &s(a.get(at).unwrap_or(&Value::Null))
                    )
                )
            } else if field == "system" {
                first_diff(&s(&expected["system"]), &s(&actual["system"]))
            } else {
                format!(
                    "\n    python: {}\n    rust:   {}",
                    cut(&expected[&field].to_string()),
                    cut(&actual[&field].to_string())
                )
            };
            report.calls.push(format!(
                "call {} ({}) {field}: {detail}",
                i + 1,
                s(&want["key"])
            ));
        }
    }
    if used.made.len() > script.calls.len() {
        report.calls.push(format!(
            "{} calls made, {} recorded",
            used.made.len(),
            script.calls.len()
        ));
    }
    // the tables
    let want_tables = sc["tables"].as_object().unwrap();
    for (table, want) in want_tables {
        if !compared(table) {
            continue;
        }
        let cols = strs(&want["columns"]);
        let rows: Vec<Vec<Value>> = want["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_array()
                    .unwrap()
                    .iter()
                    .map(|c| match scrub(&with_tool_shifts(&cell(c), &tool_shifts)) {
                        Value::String(t) => json!(fixed(&name, &unhash(&t))),
                        other => other,
                    })
                    .collect()
            })
            .collect();
        report.checked += rows.len();
        match mine.get(table) {
            None => report.tables.push(format!("{table}: no rows written")),
            Some((mcols, mrows)) => {
                if *mcols != cols {
                    report.tables.push(format!("{table}: columns {mcols:?}"));
                    continue;
                }
                let (rows, mrows) = named_diffs::named_rows(table, &cols, rows.clone(), mrows);
                for i in 0..rows.len().max(mrows.len()) {
                    if rows.get(i) != mrows.get(i) {
                        let detail = match (rows.get(i), mrows.get(i)) {
                            (Some(a), Some(b)) => {
                                let c = (0..a.len()).find(|c| a[*c] != b[*c]).unwrap_or(0);
                                format!(
                                    "{} {}",
                                    cols[c],
                                    first_diff(&a[c].to_string(), &b[c].to_string())
                                )
                            }
                            (a, b) => format!(
                                "\n    python: {}\n    rust:   {}",
                                cut(&a.map(|r| json!(r).to_string()).unwrap_or("(none)".into())),
                                cut(&b.map(|r| json!(r).to_string()).unwrap_or("(none)".into()))
                            ),
                        };
                        report.tables.push(format!("{table} row {i} {detail}"));
                        break;
                    }
                }
            }
        }
    }
    for table in mine.keys() {
        if compared(table) && !want_tables.contains_key(table) && !is_shadow(table) {
            report
                .tables
                .push(format!("{table}: written, not in the recording"));
        }
    }
    report
}

// decision: ENG-13
#[test]
fn the_recorded_chat_scenarios_through_the_program() {
    let chat = fixture("daemon/chat_scenarios.json");
    let (_folder, run_root) = RunRoot::new(recorded_root_len(&chat));
    let answers = fixture("catalog/answer_schemas.json");
    let schemas: Map<String, Value> = answers["schemas"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
        .collect();
    let only = std::env::var("CHAT_REPLAY").ok();
    let debug = std::env::var("CHAT_REPLAY_DEBUG").is_ok();
    let (mut lines, mut full, mut checked, mut orders) = (Vec::new(), 0, 0, Vec::new());
    for (i, sc) in chat["scenarios"].as_array().unwrap().iter().enumerate() {
        let name = s(&sc["name"]);
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let r = run_scenario(sc, &base_records(&name), catalogs(), i, &run_root, &schemas);
        checked += r.checked;
        orders.extend(r.engine_order.clone());
        if r.full() {
            full += 1;
            lines.push(format!("{name}: replays fully"));
            eprintln!("{name}: replays fully");
            continue;
        }
        let mut why = Vec::new();
        if let Some(stop) = &r.stopped {
            why.push(format!("stops at {stop}"));
        }
        let take = if debug { usize::MAX } else { 1 };
        why.extend(r.frames.iter().take(take).map(|f| format!("frames: {f}")));
        why.extend(
            r.calls
                .iter()
                .take(take)
                .map(|f| format!("model calls: {f}")),
        );
        why.extend(
            r.tables
                .iter()
                .take(if debug { usize::MAX } else { 3 })
                .map(|f| format!("tables: {f}")),
        );
        why.extend(
            r.entries
                .iter()
                .take(if debug { usize::MAX } else { 3 })
                .map(|f| format!("entries: {f}")),
        );
        lines.push(format!("{name}: replays partly — {}", why.join("\n  ")));
        eprintln!("{}", lines.last().unwrap());
    }
    eprintln!(
        "{full} of {} replay fully; {checked} comparisons",
        lines.len()
    );
    let clean = full == lines.len();
    lines.extend(engine_order_file(
        "engine_chat.json",
        &orders,
        only.is_some(),
        clean,
    ));
    let failures: Vec<&String> = lines
        .iter()
        .filter(|l| !l.ends_with(": replays fully"))
        .filter(|l| {
            !KNOWN_GAPS
                .iter()
                .any(|(name, gap)| l.starts_with(&format!("{name}: ")) && only_gap(l, gap))
        })
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The engine's interleavings of one test's scenarios (`engine/fixtures/chat-entries/<file>`, not Python-recorded: the
/// Rust engine's own run), written again with `UPDATE_GOLDEN=1` (a full run only), else compared: a scenario whose
/// run interleaved differently is reported (`<name>: replays partly — engine order: ...`).
fn engine_order_file(file: &str, orders: &[Value], filtered: bool, clean: bool) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/chat-entries")
        .join(file);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        if filtered || !clean {
            return vec![format!(
                "{file}: UPDATE_GOLDEN writes it from a full run where every scenario replays fully only"
            )];
        }
        let mut text = String::from("{\"about\": ");
        text += &serde_json::to_string(
            "Generated by rust/crates/ferbd/tests/chat_replay.rs (UPDATE_GOLDEN=1). Not Python-recorded: each scenario \
             as the Rust engine ran it through the real program, its chat log's inputs in the order the engine fed them \
             (`timeline`, the words of scenarios.json) and the entries it stored (`entries`).",
        )
        .unwrap();
        text += ",\n\"scenarios\": [\n";
        for (i, o) in orders.iter().enumerate() {
            text += &format!("{{\"name\": {},\n\"timeline\": [\n", o["name"]);
            let items: Vec<String> = o["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.to_string())
                .collect();
            text += &items.join(",\n");
            text += "],\n\"entries\": [\n";
            let items: Vec<String> = o["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.to_string())
                .collect();
            text += &items.join(",\n");
            text += if i + 1 == orders.len() {
                "]}\n"
            } else {
                "]},\n"
            };
        }
        text += "]}\n";
        std::fs::write(&path, text).unwrap();
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![format!("{file}: missing (UPDATE_GOLDEN=1 writes it)")];
    };
    let kept: Value = serde_json::from_str(&text).unwrap();
    let kept: HashMap<String, &Value> = kept["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (s(&o["name"]), o))
        .collect();
    let mut out = Vec::new();
    for o in orders {
        let name = s(&o["name"]);
        match kept.get(&name) {
            Some(k) if *k == o => {}
            Some(k) => {
                let (a, b) = (
                    k["timeline"].as_array().unwrap(),
                    o["timeline"].as_array().unwrap(),
                );
                let at = (0..a.len().max(b.len()))
                    .find(|i| a.get(*i) != b.get(*i))
                    .map(|i| {
                        format!(
                            "timeline item {i}: kept {} / run {}",
                            cut(&a.get(i).map(|v| v.to_string()).unwrap_or_default()),
                            cut(&b.get(i).map(|v| v.to_string()).unwrap_or_default())
                        )
                    })
                    .unwrap_or_else(|| "its entries".into());
                out.push(format!("{name}: replays partly — engine order: {at}"));
            }
            None => out.push(format!(
                "{name}: replays partly — engine order: not in {file}"
            )),
        }
    }
    out
}

/// What still differs, by scenario, with the reason (each a seam mismatch reported in engine/JOURNAL.md). Empty: a
/// tool call the global stop cancels (confirm_stop_pending) now closes the bridge's connection with no line, as
/// Python's CancelledError does (ferb-rpc's `HandlerError::cancelled`).
const KNOWN_GAPS: [(&str, &str); 0] = [];

/// The scenario's only difference is this one.
fn only_gap(line: &str, gap: &str) -> bool {
    line.split_once(" — ").is_some_and(|(_, why)| why == gap)
}

/// An answer schema the catalog names by the SHA-256 of its default form (`core_forms.default_sha256`).
fn schema_by_sha(schema: &Value) -> Option<String> {
    use sha2::Digest;
    let sha = hex::encode(sha2::Sha256::digest(
        ferb_py::json::dumps(schema).as_bytes(),
    ));
    fixture("catalog/answer_schemas.json")["schemas"]
        .as_object()?
        .iter()
        .find(|(_, v)| v["core_forms"]["default_sha256"].as_str() == Some(sha.as_str()))
        .map(|(k, _)| k.clone())
}

/// The 14 recordings of engine/fixtures/supervisor/more_scenarios.json (make_more.py): the paths the first 28 never
/// reached (the summary's fold, several asks and their card, the checklist, remember and forget, ask and wait steps, a
/// worker's question routed by the planner, the checker's runtime trouble, ...), through the program the same way.
/// The newer recordings (`supervisor/more_scenarios.json`), set up once for every run of them.
struct More {
    more: Value,
    catalogs: Value,
    schemas: Map<String, Value>,
    _folder: tempfile::TempDir,
    run_root: RunRoot,
}

impl More {
    fn new() -> More {
        // ORCH-642 (a named difference, as ferb-supervisor's replay_more): event_wait's invented topic, which the port
        // refuses at admission, replays as one FERB publishes
        let more: Value = serde_json::from_str(
            &serde_json::to_string(&fixture("supervisor/more_scenarios.json"))
                .unwrap()
                .replace("browser.download_finished", "timer.fired"),
        )
        .unwrap();
        let chat = fixture("daemon/chat_scenarios.json");
        let (_folder, run_root) = RunRoot::new(recorded_root_len(&chat));
        let answers = fixture("catalog/answer_schemas.json");
        let schemas: Map<String, Value> = answers["schemas"]
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
            .collect();
        let catalogs = resolve(&more["catalogs"]);
        More {
            more,
            catalogs,
            schemas,
            _folder,
            run_root,
        }
    }

    /// One recording through the program: whether it replays fully, else its first differences; and the engine's
    /// interleaving (`entry_order`).
    fn replay(&self, i: usize, sc: &Value, debug: bool) -> (bool, String, Option<Value>) {
        let more = &self.more;
        let name = s(&sc["name"]);
        let recs: Vec<Value> = more["records"][&name]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let mut r = resolve(r);
                if let Some(key) = r["inputs"]["inputs"].as_str() {
                    r["inputs"] = resolve(&more["inputs"][key]);
                }
                r
            })
            .collect();
        // the recorder's folder number (`/FX/NN/home`)
        let text = sc.to_string();
        let index = text
            .find("/FX/")
            .and_then(|at| text[at + 4..at + 6].parse::<usize>().ok())
            .map_or(60 + i, |n| n - 1);
        let r = run_scenario(
            sc,
            &recs,
            &self.catalogs,
            index,
            &self.run_root,
            &self.schemas,
        );
        if r.full() {
            return (true, format!("{name}: replays fully"), r.engine_order);
        }
        let mut why = Vec::new();
        if let Some(stop) = &r.stopped {
            why.push(format!("stops at {stop}"));
        }
        let take = if debug { usize::MAX } else { 1 };
        why.extend(r.frames.iter().take(take).map(|f| format!("frames: {f}")));
        why.extend(
            r.calls
                .iter()
                .take(take)
                .map(|f| format!("model calls: {f}")),
        );
        why.extend(
            r.tables
                .iter()
                .take(if debug { usize::MAX } else { 3 })
                .map(|f| format!("tables: {f}")),
        );
        why.extend(
            r.entries
                .iter()
                .take(if debug { usize::MAX } else { 3 })
                .map(|f| format!("entries: {f}")),
        );
        (
            false,
            format!("{name}: replays partly — {}", why.join("\n  ")),
            r.engine_order,
        )
    }
}

// decision: ENG-13
#[test]
fn the_more_scenarios_through_the_program() {
    let m = More::new();
    let only = std::env::var("CHAT_REPLAY").ok();
    let debug = std::env::var("CHAT_REPLAY_DEBUG").is_ok();
    let (mut lines, mut full, mut orders) = (Vec::new(), 0, Vec::new());
    for (i, sc) in m.more["scenarios"].as_array().unwrap().iter().enumerate() {
        let name = s(&sc["name"]);
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        let (whole, line, order) = m.replay(i, sc, debug);
        if whole {
            full += 1;
        }
        eprintln!("{line}");
        lines.push(line);
        orders.extend(order);
    }
    eprintln!("{full} of {} replay fully", lines.len());
    let clean = full == lines.len();
    lines.extend(engine_order_file(
        "engine_more.json",
        &orders,
        only.is_some(),
        clean,
    ));
    let failures: Vec<&String> = lines
        .iter()
        .filter(|l| !l.ends_with(": replays fully"))
        .filter(|l| !IN_PROGRESS.iter().any(|p| l.starts_with(p)))
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}

/// One recording through the program `runs` times: each run's first differences, if any.
fn every_time(name: &str, runs: usize) {
    let m = More::new();
    let (i, sc) = m.more["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .find(|(_, sc)| sc["name"] == name)
        .expect("the recording");
    let mut partly = Vec::new();
    for n in 0..runs {
        let (whole, line, _) = m.replay(i, sc, false);
        if !whole {
            partly.push(format!("run {n}: {line}"));
        }
    }
    eprintln!(
        "{name}: {} of {runs} runs replay fully",
        runs - partly.len()
    );
    assert!(partly.is_empty(), "{partly:#?}");
}

/// How many times a stress test runs its recording (`STEER_RUNS`, 100 by default).
fn stress_runs() -> usize {
    std::env::var("STEER_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
}

/// A correction that wakes a resting loop (loop_steer_finish), many times through the program on its several
/// threads: the round's span, the reply and the round's worker come in Python's order every time (`SteerHold` in
/// ferb-supervisor's loops.rs). `STEER_RUNS` sets how many (100 by default). Ignored in the gate (each run starts
/// the program); run it with `cargo test -p ferbd --test chat_replay -- --ignored a_correction_and_a_woken_loop`.
#[test]
#[ignore]
fn a_correction_and_a_woken_loop_keep_pythons_order_every_time() {
    every_time("loop_steer_finish", stress_runs());
}

/// A delegated conversation's turn (conversation_handover), many times through the program: what its thread read made
/// ready on the bus (`tool.called`, `action.observed`) reaches the chat before the hand-over's reply every time, however
/// the stream's task is woken (the wake mark, `python_wakes_here` in ferb-supervisor's tree.rs). `STEER_RUNS` sets how
/// many (100 by default); run it with `cargo test -p ferbd --test chat_replay -- --ignored a_conversation_turn`.
#[test]
#[ignore]
fn a_conversation_turn_keeps_pythons_order_every_time() {
    every_time("conversation_handover", stress_runs());
}

/// Recordings of paths not judged here yet (none: every recording is judged).
const IN_PROGRESS: [&str; 0] = [];

// ---------------------------------------------------------------------------------------------------------------
// stage 2: the chat's calls through the program (ENG-10: the conversation read like from a server)
// ---------------------------------------------------------------------------------------------------------------

/// A daemon whose scripted model answers every planner call as `plain_reply`'s one call did (a reply), its steps not
/// replayed: the stage-2 calls' tests drive it themselves.
struct Stage2 {
    _folder: tempfile::TempDir,
    _root: RunRoot,
    run: Run,
}

fn stage2() -> Stage2 {
    let chat = fixture("daemon/chat_scenarios.json");
    let (folder, run_root) = RunRoot::new(recorded_root_len(&chat));
    let sc = scenario(&chat, "plain_reply");
    let root = PathBuf::from(&run_root.root);
    let (home, user) = Run::paths(&root, "01");
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let mut script = script_of(
        &sc,
        &base_records("plain_reply"),
        catalogs(),
        &workspace.to_string_lossy(),
    );
    let times = |v: &Vec<Value>| -> Vec<Value> { (0..12).flat_map(|_| v.clone()).collect() };
    script.calls = times(&script.calls);
    script.supervise = times(&script.supervise);
    script.systems = times(&script.systems);
    script.contexts = times(&script.contexts);
    let script_path = root.join("01-script.json");
    write_script(&script_path, &script, &Used::default(), &run_root);
    let run = Run::start(&root, "01", config(&script_path, "id"));
    Stage2 {
        _folder: folder,
        _root: run_root,
        run,
    }
}

fn client(run: &Run) -> Raw {
    let mut c = Raw::connect(&run.socket()).unwrap();
    c.hello("cli").unwrap();
    c
}

/// A unary call's answer line (events of other calls on the connection skipped).
fn rpc(c: &mut Raw, id: i64, method: &str, params: Value) -> Value {
    c.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
    loop {
        let line = c.recv().expect("an answer");
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == id {
            return v;
        }
    }
}

/// A stream's events, then its last line (its end or its error).
fn streamed(c: &mut Raw, id: i64, method: &str, params: Value) -> (Vec<Value>, Value) {
    c.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
    let mut events = Vec::new();
    loop {
        let line = c.recv().expect("a stream line");
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == id {
            return (events, v);
        }
        if v["method"] == format!("{method}.event") {
            events.push(v["params"].clone());
        }
    }
}

/// The next event of an open stream on `c`.
fn next_event(c: &mut Raw) -> Value {
    let line = c.recv().expect("an event");
    let v: Value = serde_json::from_str(&line).unwrap();
    if v.get("params").is_some() {
        v["params"].clone()
    } else {
        v
    }
}

fn page_all(c: &mut Raw, id: i64) -> Vec<Value> {
    let r = rpc(c, id, "chat.entries.page", json!({"limit": 500}));
    r["result"]["entries"].as_array().unwrap().clone()
}

// decision: ENG-10
// decision: ENG-14
#[test]
fn a_repeated_submission_is_the_same_acceptance_and_starts_no_second_turn() {
    let st = stage2();
    let mut c = client(&st.run);
    let send = json!({"text": "hi there", "client_id": "phone-a", "submission_id": "sub-1"});
    let (events, end) = streamed(&mut c, 2, "chat.send", send.clone());
    assert_eq!(end["result"]["ended"], true, "{end}");
    let ack = &events[0];
    assert_eq!(ack["type"], "ack");
    for k in ["entry_id", "seq", "epoch", "submission_id"] {
        assert!(!ack[k].is_null(), "the ack carries {k}: {ack}");
    }
    assert!(events.iter().any(|e| e["type"] == "reply"), "{events:?}");
    let made = st.run.control(json!({"do": "script"}))["made"]
        .as_array()
        .unwrap()
        .len();
    // the same submission again: the same acceptance, marked a repeat, and nothing else
    let (again, end) = streamed(&mut c, 3, "chat.send", send.clone());
    assert_eq!(end["result"]["ended"], true);
    assert_eq!(again.len(), 1, "{again:?}");
    assert_eq!(again[0]["repeat"], true);
    for k in [
        "request_id",
        "entry_id",
        "seq",
        "epoch",
        "submission_id",
        "message",
    ] {
        assert_eq!(again[0][k], ack[k], "{k}");
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        st.run.control(json!({"do": "script"}))["made"]
            .as_array()
            .unwrap()
            .len(),
        made,
        "no second turn"
    );
    // the same key with other words is refused; a submission id needs its client
    let (_, refused) = streamed(
        &mut c,
        4,
        "chat.send",
        json!({"text": "something else", "client_id": "phone-a", "submission_id": "sub-1"}),
    );
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already used"));
    let (_, refused) = streamed(
        &mut c,
        5,
        "chat.send",
        json!({"text": "x", "submission_id": "sub-2"}),
    );
    assert!(refused.get("error").is_some(), "{refused}");
    // one message entry, naming its client and submission; its answer
    let entries = page_all(&mut c, 6);
    let messages: Vec<&Value> = entries.iter().filter(|e| e["kind"] == "message").collect();
    assert_eq!(messages.len(), 1, "{entries:?}");
    assert_eq!(messages[0]["id"], ack["entry_id"]);
    assert_eq!(messages[0]["client_id"], "phone-a");
    assert_eq!(messages[0]["submission_id"], "sub-1");
    assert!(entries.iter().any(|e| e["kind"] == "answer"));
    // a caller without a client id gets today's bytes: no entry fields in its ack
    let (plain, _) = streamed(
        &mut c,
        7,
        "chat.send",
        json!({"session_id": "owner", "text": "hello"}),
    );
    assert!(plain[0].get("entry_id").is_none(), "{}", plain[0]);
}

// decision: ENG-8
// decision: ENG-14
#[test]
fn two_clients_sending_at_the_same_instant_are_ordered_by_commit() {
    let st = stage2();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let sock = st.run.socket();
    let sends: Vec<_> = ["phone-a", "phone-b"]
        .iter()
        .map(|who| {
            let (gate, sock, who) = (gate.clone(), sock.clone(), who.to_string());
            std::thread::spawn(move || {
                let mut c = Raw::connect(&sock).unwrap();
                c.hello("cli").unwrap();
                gate.wait();
                let (events, _) = streamed(
                    &mut c,
                    2,
                    "chat.send",
                    json!({"text": format!("from {who}"), "client_id": who, "submission_id": "one"}),
                );
                events[0].clone()
            })
        })
        .collect();
    let acks: Vec<Value> = sends.into_iter().map(|t| t.join().unwrap()).collect();
    let (a, b) = (
        acks[0]["seq"].as_u64().unwrap(),
        acks[1]["seq"].as_u64().unwrap(),
    );
    assert_ne!(a, b, "one counter, bumped in each commit");
    let mut c = client(&st.run);
    std::thread::sleep(Duration::from_millis(500));
    let entries = page_all(&mut c, 3);
    let messages: Vec<&Value> = entries.iter().filter(|e| e["kind"] == "message").collect();
    assert_eq!(messages.len(), 2);
    // the message committed first is first in the chat and in the changes
    let first = if a < b { &acks[0] } else { &acks[1] };
    assert_eq!(messages[0]["id"], first["entry_id"]);
    let epoch = acks[0]["epoch"].clone();
    let r = rpc(
        &mut c,
        4,
        "chat.entries.changes",
        json!({"epoch": epoch, "after_seq": 0}),
    );
    let seqs: Vec<u64> = r["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
}

// decision: ENG-10
// decision: ENG-14
#[test]
fn a_second_client_watching_sees_a_turn_another_started_live() {
    let st = stage2();
    let mut w = client(&st.run);
    w.send(r#"{"jsonrpc": "2.0", "id": 9, "method": "chat.entries.watch", "params": {}}"#);
    let first = next_event(&mut w);
    assert_eq!(first["type"], "caught_up", "{first}");
    let mut c = client(&st.run);
    let (events, _) = streamed(
        &mut c,
        2,
        "chat.send",
        json!({"text": "hi there", "client_id": "mac"}),
    );
    let request = events[0]["request_id"].clone();
    let mut seen_message = false;
    let mut seen_answer = false;
    while !(seen_message && seen_answer) {
        let ev = next_event(&mut w);
        if ev["type"] == "change" {
            assert!(ev.get("catchup").is_none());
            let e = &ev["entry"];
            seen_message |= e["kind"] == "message" && e["request_id"] == request;
            seen_answer |= e["kind"] == "answer" && e["request_id"] == request;
        }
    }
}

// decision: ENG-10
// decision: ENG-14
#[test]
fn a_returning_watcher_catches_up_with_exactly_what_it_missed() {
    let st = stage2();
    let mut c = client(&st.run);
    let mut w = client(&st.run);
    w.send(r#"{"jsonrpc": "2.0", "id": 9, "method": "chat.entries.watch", "params": {}}"#);
    let up = next_event(&mut w);
    let epoch = up["epoch"].clone();
    let (events, _) = streamed(
        &mut c,
        2,
        "chat.send",
        json!({"text": "hi there", "client_id": "mac"}),
    );
    let request = events[0]["request_id"].clone();
    let mut cursor = 0;
    loop {
        let ev = next_event(&mut w);
        if ev["type"] == "change" {
            cursor = cursor.max(ev["entry"]["seq"].as_u64().unwrap());
            if ev["entry"]["kind"] == "answer" && ev["entry"]["request_id"] == request {
                break;
            }
        }
    }
    drop(w);
    std::thread::sleep(Duration::from_millis(300));
    // more happens while it is away
    let (more, _) = streamed(
        &mut c,
        3,
        "chat.send",
        json!({"text": "and again", "client_id": "mac"}),
    );
    assert_eq!(more[0]["type"], "ack");
    std::thread::sleep(Duration::from_millis(500));
    let missed = rpc(
        &mut c,
        4,
        "chat.entries.changes",
        json!({"epoch": epoch, "after_seq": cursor, "limit": 500}),
    );
    let want: Vec<Value> = missed["result"]["entries"].as_array().unwrap().clone();
    assert!(!want.is_empty());
    let mut back = client(&st.run);
    back.send(
        &json!({"jsonrpc": "2.0", "id": 9, "method": "chat.entries.watch",
                "params": {"epoch": epoch, "after_seq": cursor}})
        .to_string(),
    );
    let mut got = Vec::new();
    loop {
        let ev = next_event(&mut back);
        if ev["type"] == "caught_up" {
            assert_eq!(ev["head"], missed["result"]["head"]);
            break;
        }
        assert_eq!(ev["catchup"], true, "{ev}");
        got.push(ev["entry"].clone());
    }
    assert_eq!(
        got, want,
        "each missed entry once, at its latest state, in seq order"
    );
    let ids: BTreeSet<String> = got.iter().map(|e| s(&e["id"])).collect();
    assert_eq!(ids.len(), got.len());
    assert!(got.iter().all(|e| e["seq"].as_u64().unwrap() > cursor));
}

// decision: ENG-10
// decision: ENG-14
#[test]
fn a_cursor_of_another_epoch_must_reset_and_one_ahead_is_told() {
    let st = stage2();
    let mut c = client(&st.run);
    let (events, _) = streamed(
        &mut c,
        2,
        "chat.send",
        json!({"text": "hi there", "client_id": "mac"}),
    );
    let epoch = events[0]["epoch"].clone();
    let r = rpc(
        &mut c,
        3,
        "chat.entries.changes",
        json!({"epoch": "not-this-one", "after_seq": 1}),
    );
    assert_eq!(r["result"]["reset_required"], true, "{r}");
    assert_eq!(r["result"]["epoch"], epoch);
    let r = rpc(
        &mut c,
        4,
        "chat.entries.changes",
        json!({"epoch": epoch, "after_seq": 999_999}),
    );
    assert_eq!(r["result"]["cursor_ahead"], true, "{r}");
    let (frames, end) = streamed(
        &mut c,
        5,
        "chat.entries.watch",
        json!({"epoch": "not-this-one", "after_seq": 3}),
    );
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["type"], "reset_required");
    assert_eq!(end["result"]["ended"], true, "{end}");
}

// decision: ENG-10
// decision: ENG-14
#[test]
fn a_long_body_comes_in_pieces_and_max_bytes_cuts_an_entry() {
    let st = stage2();
    let mut c = client(&st.run);
    let long: String = (0..1200).map(|i| format!("w{i} ")).collect::<String>();
    let full = long.trim_end().to_string();
    let (events, _) = streamed(
        &mut c,
        2,
        "chat.send",
        json!({"text": long, "client_id": "mac"}),
    );
    let id = events[0]["entry_id"].clone();
    std::thread::sleep(Duration::from_millis(300));
    let mut text = String::new();
    let mut offset = 0;
    for n in 0.. {
        let r = rpc(
            &mut c,
            10 + n,
            "chat.entries.get",
            json!({"id": id, "offset": offset, "max_chars": 1000}),
        );
        let e = &r["result"]["entry"];
        text += e["body"]["text"].as_str().unwrap();
        assert_eq!(e["text_piece"]["full_chars"], full.chars().count());
        offset += e["text_piece"]["chars"].as_u64().unwrap();
        if e["text_piece"]["more"] == false {
            break;
        }
    }
    assert_eq!(text, full);
    let r = rpc(&mut c, 3, "chat.entries.page", json!({"max_bytes": 4096}));
    let entries = r["result"]["entries"].as_array().unwrap();
    assert_eq!(r["result"]["more"], true);
    assert!(
        r["result"].to_string().len() <= 4096,
        "{}",
        r["result"].to_string().len()
    );
    let cut = entries
        .iter()
        .find(|e| e["id"] == id)
        .expect("the long message, cut");
    assert_eq!(cut["body_cut"]["full_chars"], full.chars().count());
    assert!(cut["body_cut"]["chars"].as_u64().unwrap() < full.chars().count() as u64);
}

// decision: ENG-10
// decision: ENG-14
#[test]
fn a_clients_marks_only_move_forward() {
    let st = stage2();
    let mut c = client(&st.run);
    let r = rpc(
        &mut c,
        2,
        "chat.entries.mark",
        json!({"client_id": "phone-a", "delivered_seq": 5, "read_pos": 3}),
    );
    assert_eq!(r["result"]["marks"]["delivered_seq"], 5, "{r}");
    let r = rpc(
        &mut c,
        3,
        "chat.entries.mark",
        json!({"client_id": "phone-a", "delivered_seq": 2, "read_pos": 7}),
    );
    assert_eq!(r["result"]["marks"]["delivered_seq"], 5);
    assert_eq!(r["result"]["marks"]["read_pos"], 7);
    let r = rpc(&mut c, 4, "chat.entries.mark", json!({"delivered_seq": 2}));
    assert!(r.get("error").is_some(), "a mark needs its client: {r}");
    let r = rpc(&mut c, 5, "chat.entries.days", json!({}));
    assert!(r["result"]["days"].is_array(), "{r}");
}

/// GATE-604 through the real daemon: an identifier in the owner's message goes to the owner's saved secrets. A test
/// daemon reaches no Keychain, so the save fails: the message is only masked, as before (no `sealed` in the ack, no
/// handle file), and the journal names the kind that could not be kept, never the value.
// decision: GATE-604
#[test]
fn an_identifier_the_keychain_cannot_keep_is_only_masked_and_its_kind_journaled() {
    let st = stage2();
    let mut c = client(&st.run);
    let (events, _) = streamed(
        &mut c,
        2,
        "chat.send",
        json!({"text": "my PAN is ABCPE1234F", "client_id": "mac"}),
    );
    let ack = &events[0];
    assert_eq!(ack["type"], "ack", "{ack}");
    assert_eq!(ack["message"], "my PAN is [redacted]");
    assert_eq!(ack["hidden"], json!(["PAN"]));
    assert!(ack.get("sealed").is_none(), "{ack}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!st.run.home.join("owner-secret-handles.json").exists());
    let conn = rusqlite::Connection::open_with_flags(
        st.run.home.join("ferb.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT kind, data_json FROM journal WHERE kind LIKE 'identifier.%'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![(
            "identifier.seal_failed".to_string(),
            r#"{"kind": "PAN"}"#.to_string()
        )]
    );
    let all: String = conn
        .query_row("SELECT group_concat(data_json) FROM journal", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .unwrap()
        .unwrap_or_default();
    assert!(!all.contains("ABCPE1234F"));
}
