//! The chat as entries (stage 2, ENG-8): `flow::entries::ChatProjector` folded over every recorded chat scenario, and
//! its rules.
//!
//! **These goldens are not Python-recorded.** Python has no counterpart: the entries are the engine's own projection of
//! what FERB.app's reducer keeps today. `engine/fixtures/chat-entries/scenarios.json` is written by this test from the
//! recorded scenarios (`UPDATE_GOLDEN=1 cargo test -p ferb-supervisor --test chat_entries` writes it again) and
//! compared byte for byte otherwise; FERB.app's own reducer is the reference it is checked against
//! (`apps/mac/FERBNotch/Tests/NotchCoreTests/EntryFoldTests.swift`: the same timelines through the app's code give the
//! same transcript as the entries folded).
//!
//! Each scenario's timeline comes from the recordings of the real Python daemon (engine/fixtures/daemon/chat_scenarios.json,
//! engine/fixtures/supervisor/control_scenarios.json and more_scenarios.json): every `chat.send` stream (its `ack` when
//! the message was sent, the rest of its frames where its stream ended), the `task.result` the engine publishes with
//! every outcome (chat.rs `task_result`), the results, notices and questions `chat.status` read back, the answers given
//! (`approval.resolved`, with the facts of the scenario's `question.answered` row) and the reminder banners. A recording
//! keeps each connection's lines in order but not their order across connections; the timeline's order (a stream's
//! frames where it ended) is a choice, the same for the engine and the app.

// the order rules every conversation's entries keep (shared with ferbd's chat_replay)
mod support_entries;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use ferb_core::chat_entry::{
    AnswerBody, Entry, EntryKind, EntryState, MessageBody, Placement, QuestionBody,
};
use ferb_supervisor::flow::entries::{ChatProjector, Input, Projected, Seams};
use serde_json::{json, Map, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

const SOURCES: [&str; 3] = [
    "daemon/chat_scenarios.json",
    "supervisor/control_scenarios.json",
    "supervisor/more_scenarios.json",
];
const BODIES: [&str; 3] = [
    "daemon/bodies.json",
    "supervisor/control_bodies.json",
    "supervisor/more_bodies.json",
];
const GOLDEN: &str = "chat-entries/scenarios.json";

fn read(path: &str) -> Value {
    let text = std::fs::read_to_string(fixtures().join(path)).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A recorded line: its text, or the body it names (`{"sha256"}`).
fn line(v: &Value, bodies: &Map<String, Value>) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(m) => bodies[m["sha256"].as_str().unwrap()]
            .as_str()
            .unwrap()
            .to_string(),
        _ => panic!("a line is a string or a body"),
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// The deliberate differences from Python (ENG-6) the engine's frames carry, applied to a recorded frame's line before
/// it enters the timeline: (scenario, recorded, the port's); the owner-facing ones of tests/replay/harness.rs `FIXES`
/// (the others change only what a model reads). Global stop and the forget question (journal 43, 50): FERB's own
/// words are not tagged "(from the task)". ORCH-649: a conversation change from chat is said by who the chat is with,
/// not as "Conversation <action>: <id>".
const FIXES: [(&str, &str, &str); 4] = [
    (
        "global_stop",
        "(from the task) FERB was stopped",
        "FERB was stopped",
    ),
    (
        "remember_then_forget",
        "(from the task) FERB deletes:",
        "FERB deletes:",
    ),
    (
        "displaced_question",
        "(from the task) FERB deletes:",
        "FERB deletes:",
    ),
    (
        "conversation_change",
        "Conversation pause: id-000021:chat",
        "Paused the chat with Robin.",
    ),
];

fn fixed(scenario: &str, text: String) -> String {
    FIXES
        .iter()
        .filter(|(n, _, _)| *n == scenario)
        .fold(text, |t, (_, a, b)| t.replace(a, b))
}

/// One scenario as a timeline of the conversation: what was sent, the frames, the events.
struct Scenario {
    name: String,
    source: String,
    timeline: Vec<Value>,
}

/// The timeline of one recorded scenario (see the file's comment).
fn timeline(scenario: &Value, bodies: &Map<String, Value>) -> Vec<Value> {
    let steps = scenario["steps"].as_array().unwrap();
    let name = scenario["name"].as_str().unwrap_or_default();
    // the journal's question.answered rows: the facts approval.resolved goes with
    let mut answered: BTreeMap<String, Value> = BTreeMap::new();
    if let Some(j) = scenario["tables"].get("journal") {
        let cols: Vec<&str> = j["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        for row in j["rows"].as_array().unwrap() {
            let row: Map<String, Value> = cols
                .iter()
                .map(|c| c.to_string())
                .zip(row.as_array().unwrap().iter().cloned())
                .collect();
            if row["kind"] != "question.answered" {
                continue;
            }
            let data: Value = serde_json::from_str(&line(&row["data_json"], bodies)).unwrap();
            if let Some(a) = s(&data, "approval_id") {
                answered.insert(a, data);
            }
        }
    }
    // each stream's lines, by the step that ended it: `conn` streams in order
    let mut ends: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, st) in steps.iter().enumerate() {
        if matches!(
            st["do"].as_str(),
            Some("stream_end" | "stream_end_at_shutdown")
        ) {
            ends.entry(s(st, "conn").unwrap()).or_default().push(i);
        }
    }
    let frames_of = |i: usize| -> Vec<Value> {
        steps[i]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| l.get("recv"))
            .map(|l| serde_json::from_str::<Value>(&fixed(name, line(l, bodies))).unwrap())
            .filter(|m| m["method"] == "chat.send.event")
            .map(|m| m["params"].clone())
            .collect()
    };
    let mut out = Vec::new();
    let mut at = "2026-01-01T09:00:00+00:00".to_string();
    let mut streams: HashMap<String, (usize, String, String)> = HashMap::new(); // end step → (stream, session, request)
    let mut sessions: HashMap<String, String> = HashMap::new(); // request → session
    let mut first_session: Option<String> = None;
    let mut taken: HashMap<String, usize> = HashMap::new();
    let mut seen_results: HashSet<String> = HashSet::new();
    let mut seen_notices: HashSet<String> = HashSet::new();
    let mut seen_questions: HashSet<String> = HashSet::new();
    let mut resolved: HashSet<String> = HashSet::new();
    let mut count = 0usize;
    let route =
        |data: &Value, sessions: &HashMap<String, String>, first: &Option<String>| -> String {
            s(data, "request_id")
                .and_then(|r| sessions.get(&r).cloned())
                .or_else(|| first.clone())
                .unwrap_or_else(|| "s1".into())
        };
    for st in steps {
        let conn = s(st, "conn").unwrap_or_default();
        match st["do"].as_str().unwrap() {
            "stream" => {
                let sent: Value =
                    serde_json::from_str(&line(&st["lines"][0]["send"], bodies)).unwrap();
                if sent["method"] != "chat.send" {
                    continue;
                }
                let n = taken.entry(conn.clone()).or_default();
                let end = ends[&conn][*n];
                *n += 1;
                let frames = frames_of(end);
                let Some(ack) = frames.first().filter(|f| f["type"] == "ack") else {
                    continue; // refused before its ack: no message was accepted
                };
                let session = s(&sent["params"], "session_id").unwrap_or_else(|| "default".into());
                let request = s(ack, "request_id").unwrap();
                first_session.get_or_insert(session.clone());
                sessions.insert(request.clone(), session.clone());
                let stream = count;
                count += 1;
                at = s(ack, "at").unwrap_or(at);
                let paths = sent["params"]
                    .get("input_paths")
                    .cloned()
                    .unwrap_or(json!([]));
                out.push(json!({"do": "send", "stream": stream, "session": session, "request_id": request,
                                "text": sent["params"]["text"], "input_paths": paths, "at": at}));
                out.push(
                    json!({"do": "frame", "stream": stream, "session": session, "frame": ack}),
                );
                streams.insert(format!("{conn}#{end}"), (stream, session, request));
            }
            "stream_end" | "stream_end_at_shutdown" => {
                let me = steps.iter().position(|x| std::ptr::eq(x, st)).unwrap();
                let Some((stream, session, _)) = streams.get(&format!("{conn}#{me}")).cloned()
                else {
                    continue;
                };
                for frame in frames_of(me).into_iter().skip(1) {
                    if let Some(a) = s(&frame, "at") {
                        at = a;
                    }
                    let outcome = frame["type"] == "outcome";
                    out.push(json!({"do": "frame", "stream": stream, "session": session, "frame": frame}));
                    if outcome {
                        // chat.rs `task_result`: the outcome, published as task.result
                        let mut data = frame.clone();
                        data["shown"] = json!(true);
                        if let Some(t) = s(&frame, "task_id") {
                            seen_results.insert(t);
                        }
                        out.push(json!({"do": "bus", "session": session, "topic": "task.result", "data": data, "at": at}));
                    }
                    if frame["type"] == "event" && frame["topic"] == "approval.requested" {
                        if let Some(a) = s(&frame["data"], "approval_id") {
                            seen_questions.insert(a);
                        }
                    }
                }
                out.push(json!({"do": "end", "stream": stream, "session": session}));
            }
            "call" => {
                let lines = st["lines"].as_array().unwrap();
                let mut sent: Option<Value> = None;
                for l in lines {
                    if let Some(x) = l.get("send") {
                        sent = Some(serde_json::from_str(&line(x, bodies)).unwrap());
                        continue;
                    }
                    let got: Value = serde_json::from_str(&line(&l["recv"], bodies)).unwrap();
                    let Some(sent) = sent.as_ref().filter(|q| q.get("id") == got.get("id")) else {
                        continue;
                    };
                    let method = sent["method"].as_str().unwrap_or("");
                    let result = &got["result"];
                    if method == "chat.status" && result.is_object() {
                        for q in result["approvals"].as_array().into_iter().flatten() {
                            let a = s(q, "approval_id").unwrap_or_default();
                            if seen_questions.insert(a) {
                                let session = route(q, &sessions, &first_session);
                                out.push(json!({"do": "bus", "session": session, "topic": "approval.requested", "data": q, "at": at}));
                            }
                        }
                        for r in result["results"].as_array().into_iter().flatten() {
                            let t = s(r, "task_id").unwrap_or_default();
                            if seen_results.insert(t) {
                                let session = route(r, &sessions, &first_session);
                                out.push(json!({"do": "result", "session": session, "data": r}));
                            }
                        }
                        for n in result["notices"].as_array().into_iter().flatten() {
                            let id = s(n, "id").unwrap_or_default();
                            if seen_notices.insert(id) {
                                let session = route(n, &sessions, &first_session);
                                out.push(json!({"do": "notice", "session": session, "data": n}));
                            }
                        }
                    }
                    if ["chat.approve", "chat.answer", "chat.correct"].contains(&method) {
                        if let Some(a) = s(&sent["params"], "approval_id") {
                            if result.is_object() && resolved.insert(a.clone()) {
                                let mut data = answered.get(&a).cloned().unwrap_or_else(
                                    || json!({"approval_id": a, "state": result["state"]}),
                                );
                                data["approval_id"] = json!(a);
                                let session = first_session.clone().unwrap_or_else(|| "s1".into());
                                out.push(json!({"do": "bus", "session": session, "topic": "approval.resolved", "data": data, "at": at}));
                            }
                        }
                    }
                }
            }
            "stream_cut" => {
                for l in st["lines"].as_array().unwrap() {
                    let Some(x) = l.get("recv") else { continue };
                    let text = line(x, bodies);
                    let Ok(m) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if m["method"] == "notifications.subscribe.event" {
                        let session = first_session.clone().unwrap_or_else(|| "s1".into());
                        out.push(
                            json!({"do": "notification", "session": session, "item": m["params"]}),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    // questions answered by no call (a stop or a steer set them aside): at the end
    for (a, data) in answered {
        if seen_questions.contains(&a) && resolved.insert(a.clone()) {
            let session = first_session.clone().unwrap_or_else(|| "s1".into());
            out.push(json!({"do": "bus", "session": session, "topic": "approval.resolved", "data": data, "at": at}));
        }
    }
    out
}

fn scenarios() -> Vec<Scenario> {
    let mut bodies = Map::new();
    for b in BODIES {
        if let Value::Object(m) = read(b) {
            bodies.extend(m);
        }
    }
    let mut out = Vec::new();
    for source in SOURCES {
        for sc in read(source)["scenarios"].as_array().unwrap() {
            out.push(Scenario {
                name: s(sc, "name").unwrap(),
                source: source.to_string(),
                timeline: timeline(sc, &bodies),
            });
        }
    }
    out.extend(invented());
    out
}

/// The invented scenario of the live case of 2026-11-15 (APP-919).
const STOP_TWO_TURNS: &str = "stop_beside_new_work_two_turns";
/// What the stopped task had found, as code recorded its end (ORCH-611).
const STOPPED_SUMMARY: &str =
    "Stopped, as you asked. This is where things stood:\n\nThe app is complete, but the \
     report is not ready to submit.\n\nSites contacted: 2 sites";
/// The new task's plan line under the stop message (ORCH-638: the stop's words come first, in text and receipts).
const FILES_LINE: &str = "I'm listing every file the assignment workers made.";

/// Timelines written here for chat cases found live that no recording has (ENG-18: the Python daemon is gone, ENG-16),
/// in the steps a recorded timeline has; they come after the 71 recorded scenarios.
fn invented() -> Vec<Scenario> {
    vec![Scenario {
        name: STOP_TWO_TURNS.into(),
        source: "invented: chat_entries.rs, the live case of 2026-11-15".into(),
        timeline: stop_beside_new_work_two_turns(true),
    }]
}

/// The live case of 2026-11-15 (APP-919): a task the owner restarted with a second message (the same task now runs
/// under two turns: the first already answered "Stopped" below, the second bound by its plan), then a third message
/// that stops it and asks for new work (ORCH-638). The stopped task's end comes on the second turn's stream naming the
/// third message (`told_in`, ORCH-579); the third message's plan says the stop first. `plan_first`: the new task's plan
/// is fed before the stopped task's end (the engine's order that night); otherwise after it.
fn stop_beside_new_work_two_turns(plan_first: bool) -> Vec<Value> {
    let at = |m: u32| format!("2026-01-01T09:{m:02}:00+00:00");
    let send = |stream: u64, request: &str, text: &str, m: u32| {
        vec![
            json!({"do": "send", "stream": stream, "session": "s1", "request_id": request, "text": text,
                   "input_paths": [], "at": at(m)}),
            json!({"do": "frame", "stream": stream, "session": "s1",
                   "frame": {"type": "ack", "request_id": request, "message": text, "at": at(m)}}),
        ]
    };
    let frame = |stream: u64, f: Value| json!({"do": "frame", "stream": stream, "session": "s1", "frame": f});
    let end = |stream: u64| json!({"do": "end", "stream": stream, "session": "s1"});
    let result = |f: &Value, m: u32| {
        let mut data = f.clone();
        data["shown"] = json!(true);
        json!({"do": "bus", "session": "s1", "topic": "task.result", "data": data, "at": at(m)})
    };
    let goal = "Complete the three course assignments";
    let first_end = json!({"type": "outcome", "task_id": "t-assign", "status": "cancelled", "summary": "stopped by you",
        "answer": "Stopped, as you asked.\n\nSites contacted: 1 site", "verified": false, "artifacts": [],
        "lines": ["Sites contacted: 1 site"], "goal": goal, "name": "Course assignments", "request_id": "r1",
        "at": at(5)});
    let stopped = json!({"type": "outcome", "task_id": "t-assign", "status": "cancelled", "summary": "stopped by you",
        "answer": STOPPED_SUMMARY, "verified": false, "artifacts": ["/FX/work/app.zip", "/FX/work/report.pdf"],
        "lines": ["Sites contacted: 2 sites"], "told_in": "r3", "goal": goal, "name": "Course assignments",
        "request_id": "r2", "at": at(20)});
    let files_plan = frame(
        2,
        json!({"type": "plan", "task_id": "t-files", "text": format!("{STOPPED_SUMMARY}\n\n{FILES_LINE}"),
               "goal": "List every file the assignment workers made", "name": "List generated files",
               "plan_line": FILES_LINE, "lines": [], "receipts": [STOPPED_SUMMARY], "request_id": "r3", "at": at(20)}),
    );
    let files_end = json!({"type": "outcome", "task_id": "t-files", "status": "done",
        "summary": "Two files: /FX/work/app.zip and /FX/work/report.pdf.",
        "answer": "Two files: /FX/work/app.zip and /FX/work/report.pdf.", "verified": true, "artifacts": [],
        "goal": "List every file the assignment workers made", "name": "List generated files", "request_id": "r3",
        "at": at(21)});
    let mut out = send(0, "r1", "do the three course assignments", 0);
    out.push(frame(
        0,
        json!({"type": "plan", "task_id": "t-assign", "text": "Working on the three assignments.", "goal": goal,
               "name": "Course assignments", "plan_line": "Working on the three assignments.", "lines": [],
               "request_id": "r1", "at": at(0)}),
    ));
    // the second message restarts the same task: its first run's end, then the second turn's plan for it
    out.extend(send(
        1,
        "r2",
        "start again with three workers, one per assignment",
        5,
    ));
    out.push(frame(0, first_end.clone()));
    out.push(result(&first_end, 5));
    out.push(end(0));
    out.push(frame(
        1,
        json!({"type": "plan", "task_id": "t-assign", "text": "I'm restarting it with three workers.", "goal": goal,
               "name": "Course assignments", "plan_line": "I'm restarting it with three workers.", "lines": [],
               "request_id": "r2", "at": at(5)}),
    ));
    // the third message stops it and asks for new work
    out.extend(send(
        2,
        "r3",
        "stop all the tasks. list where all the files they made are",
        20,
    ));
    if plan_first {
        out.push(files_plan.clone());
    }
    out.push(frame(1, stopped.clone()));
    out.push(result(&stopped, 20));
    out.push(end(1));
    if !plan_first {
        out.push(files_plan);
    }
    out.push(frame(2, files_end.clone()));
    out.push(result(&files_end, 21));
    out.push(end(2));
    out
}

/// The seams: the timeline's clock, ids counted per scenario, one change counter per conversation.
struct Fixed {
    at: String,
    ids: u64,
    seq: u64,
}

impl Seams for Fixed {
    fn now(&mut self) -> String {
        self.at.clone()
    }
    fn new_id(&mut self) -> String {
        self.ids += 1;
        format!("ce-{:06}", self.ids)
    }
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// One timeline run: per item, the conversation and what it made.
struct Run {
    made: Vec<(String, Value, Projected)>,
    entries: BTreeMap<String, Vec<Entry>>,
}

fn run(timeline: &[Value]) -> Run {
    let mut projectors: BTreeMap<String, (ChatProjector, Fixed)> = BTreeMap::new();
    let mut ids = 0u64;
    let mut at = "2026-01-01T09:00:00+00:00".to_string();
    let mut requests: HashMap<u64, String> = HashMap::new();
    let mut made = Vec::new();
    for item in timeline {
        let session = s(item, "session").unwrap();
        let (projector, seams) = projectors.entry(session.clone()).or_insert_with(|| {
            (
                ChatProjector::new(&session, 1),
                Fixed {
                    at: at.clone(),
                    ids: 0,
                    seq: 0,
                },
            )
        });
        if let Some(a) = s(item, "at").or_else(|| item.get("frame").and_then(|f| s(f, "at"))) {
            at = a;
        }
        seams.at = at.clone();
        seams.ids = ids;
        let stream = item.get("stream").and_then(Value::as_u64);
        let out = match item["do"].as_str().unwrap() {
            "send" => {
                requests.insert(stream.unwrap(), s(item, "request_id").unwrap());
                continue;
            }
            "frame" => {
                let request = requests[&stream.unwrap()].clone();
                let mut frame = item["frame"].clone();
                if frame["type"] == "ack" {
                    // what the engine knows at its ack: the message's files
                    let paths = timeline
                        .iter()
                        .find(|x| x["do"] == "send" && x["stream"] == item["stream"])
                        .map(|x| x["input_paths"].clone())
                        .unwrap_or(json!([]));
                    frame["input_paths"] = paths;
                }
                projector.apply(
                    Input::Frame {
                        request_id: &request,
                        frame: &frame,
                    },
                    seams,
                )
            }
            "end" => {
                let request = requests[&stream.unwrap()].clone();
                projector.apply(
                    Input::StreamEnded {
                        request_id: &request,
                    },
                    seams,
                )
            }
            "bus" => projector.apply(
                Input::Bus {
                    topic: item["topic"].as_str().unwrap(),
                    data: &item["data"],
                },
                seams,
            ),
            "result" => projector.apply(
                Input::Bus {
                    topic: "task.result",
                    data: &item["data"],
                },
                seams,
            ),
            "notice" => projector.apply(
                Input::Bus {
                    topic: "attention.notice",
                    data: &item["data"],
                },
                seams,
            ),
            "notification" => projector.apply(Input::Notification(&item["item"]), seams),
            other => panic!("unknown timeline step {other}"),
        };
        ids = seams.ids;
        made.push((session, item.clone(), out));
    }
    let entries = projectors
        .into_iter()
        .map(|(k, (p, _))| (k, p.entries()))
        .collect();
    Run { made, entries }
}

/// The golden file: each scenario's timeline and the entries it ends with, one JSON value per line.
fn golden(all: &[Scenario]) -> String {
    let mut out = String::from("{\"about\": ");
    out += &serde_json::to_string(
        "Generated by rust/crates/ferb-supervisor/tests/chat_entries.rs (UPDATE_GOLDEN=1). Not Python-recorded: Python \
         has no counterpart. Each scenario's timeline is built from the recorded scenarios named in `source`; `entries` \
         are what ChatProjector made of it, each entry as it stands at the end, per conversation in creation order. A \
         `source` starting \"invented:\" is a timeline written in chat_entries.rs for a case found live (ENG-18).",
    )
    .unwrap();
    out += ",\n\"scenarios\": [\n";
    for (i, sc) in all.iter().enumerate() {
        let r = run(&sc.timeline);
        let changes: usize = r.made.iter().map(|m| m.2.changes.len()).sum();
        out += &format!(
            "{{\"name\": {}, \"source\": {}, \"changes\": {changes},\n\"timeline\": [\n",
            serde_json::to_string(&sc.name).unwrap(),
            serde_json::to_string(&sc.source).unwrap()
        );
        let items: Vec<String> = sc
            .timeline
            .iter()
            .map(|t| serde_json::to_string(t).unwrap())
            .collect();
        out += &items.join(",\n");
        out += "],\n\"entries\": [\n";
        let entries: Vec<String> = r
            .entries
            .values()
            .flatten()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        out += &entries.join(",\n");
        out += if i + 1 == all.len() { "]}\n" } else { "]},\n" };
    }
    out += "]}\n";
    out
}

// decision: ENG-9
// decision: ENG-18
#[test]
fn the_recorded_scenarios_fold_into_the_golden_entries() {
    let all = scenarios();
    assert_eq!(
        all.len(),
        72,
        "28 daemon, 10 control and 33 more recorded scenarios, and 1 invented (ENG-18)"
    );
    let made = golden(&all);
    let path = fixtures().join(GOLDEN);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &made).unwrap();
    }
    let kept = std::fs::read_to_string(&path).expect("the golden file (UPDATE_GOLDEN=1 writes it)");
    assert!(
        kept == made,
        "the entries differ from {GOLDEN}; UPDATE_GOLDEN=1 writes them again, then review the diff"
    );
}

/// Live frames store nothing and come back live.
const LIVE: [&str; 11] = [
    "progress",
    "step",
    "subtask",
    "verification",
    "memory",
    "timer",
    "plan_delta",
    "plan_abort",
    "plan_end",
    "reply_delta",
    "reply_abort",
];

// decision: ENG-8
// decision: ENG-9
#[test]
fn every_rule_holds_over_every_scenario() {
    let mut acks = 0;
    let mut live = 0;
    for sc in scenarios() {
        let r = run(&sc.timeline);
        let ctx = |what: &str| format!("{}: {what}", sc.name);
        // every ack makes exactly one message (one per request)
        for (session, entries) in &r.entries {
            let sent: Vec<String> = sc
                .timeline
                .iter()
                .filter(|t| {
                    t["do"] == "frame"
                        && t["frame"]["type"] == "ack"
                        && s(t, "session").as_deref() == Some(session)
                })
                .map(|t| s(&t["frame"], "request_id").unwrap())
                .collect();
            acks += sent.len();
            let messages: Vec<String> = entries
                .iter()
                .filter(|e| e.kind == EntryKind::Message)
                .map(|e| e.request_id.clone().unwrap())
                .collect();
            assert_eq!(messages, sent, "{}", ctx("one message per ack, in order"));
        }
        // nothing live is stored
        for (_, item, out) in &r.made {
            if item["do"] == "frame" {
                let ty = item["frame"]["type"].as_str().unwrap_or("");
                let topic = item["frame"]["topic"].as_str().unwrap_or("");
                if LIVE.contains(&ty) || (ty == "event" && topic != "approval.requested") {
                    live += 1;
                    assert!(
                        out.changes.is_empty(),
                        "{}",
                        ctx(&format!("a {ty} frame stored something"))
                    );
                }
            }
        }
        // seq only grows and is unique; pos is given once; a revision bumps rev by one and changes something stored
        let mut by_session: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
        for (session, _, out) in &r.made {
            for c in &out.changes {
                assert_eq!(c.seq, c.entry.seq);
                by_session
                    .entry(session.clone())
                    .or_default()
                    .push(&c.entry);
            }
        }
        for changes in by_session.values() {
            let mut last = 0;
            let mut seen: HashMap<&str, &Entry> = HashMap::new();
            let mut positions: HashMap<u64, &str> = HashMap::new();
            for e in changes {
                assert!(e.seq > last, "{}", ctx("seq grows"));
                last = e.seq;
                match seen.get(e.id.as_str()) {
                    None => {
                        assert_eq!(e.rev, 1, "{}", ctx("a new entry is rev 1"));
                        assert!(
                            positions.insert(e.pos, &e.id).is_none(),
                            "{}",
                            ctx("pos is unique")
                        );
                    }
                    Some(before) => {
                        assert_eq!(e.rev, before.rev + 1, "{}", ctx("rev counts revisions"));
                        assert_eq!(e.pos, before.pos, "{}", ctx("pos never changes"));
                        assert_eq!(e.created_at, before.created_at);
                        assert!(
                            !e.same_content(before),
                            "{}",
                            ctx(&format!("{} revised with nothing changed", e.id))
                        );
                    }
                }
                seen.insert(&e.id, e);
            }
        }
        for entries in r.entries.values() {
            for e in entries {
                // a merged message names the request it was read with (ORCH-529)
                if e.kind == EntryKind::Message && e.state == EntryState::Merged {
                    assert!(e.body_as::<MessageBody>().unwrap().merged_into.is_some());
                }
                // an answer names its message, and a late one says what it shows
                if e.kind == EntryKind::Answer {
                    let a: AnswerBody = e.body_as().unwrap();
                    assert!(e.placement.is_some());
                    assert_eq!(
                        e.placement == Some(Placement::Below),
                        a.part.is_some(),
                        "{}",
                        ctx("a late answer's part")
                    );
                    if a.name.is_none() && a.goal.is_none() && e.request_id.is_some() {
                        assert!(
                            e.answers.is_some() || a.from == "task",
                            "{}",
                            ctx("an answer names its message")
                        );
                    }
                }
            }
        }
    }
    assert_eq!(
        acks, 104,
        "every ack of the 72 scenarios (101 recorded, 3 invented)"
    );
    assert_eq!(
        live, 669,
        "the live frames of the 72 scenarios (the invented one has none)"
    );
}

// ---- the rules on invented frames ----

struct Clock {
    n: u64,
    seq: u64,
}

impl Seams for Clock {
    fn now(&mut self) -> String {
        "2026-02-03T10:00:00+05:30".into()
    }
    fn new_id(&mut self) -> String {
        self.n += 1;
        format!("e{}", self.n)
    }
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

fn frame(p: &mut ChatProjector, c: &mut Clock, request: &str, f: Value) -> Projected {
    p.apply(
        Input::Frame {
            request_id: request,
            frame: &f,
        },
        c,
    )
}

fn by_kind(p: &ChatProjector, kind: EntryKind) -> Vec<Entry> {
    p.entries().into_iter().filter(|e| e.kind == kind).collect()
}

// decision: APP-580
#[test]
fn an_answer_to_a_message_the_chat_no_longer_ends_on_goes_below_and_names_it() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "count my files"}),
    );
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "plan", "task_id": "t1", "text": "Counting.", "request_id": "r1"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "ack", "request_id": "r2", "message": "hi"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "reply", "text": "Hello.", "request_id": "r2"}),
    );
    let out = frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "outcome", "task_id": "t1", "status": "done", "answer": "3 files.", "request_id": "r1"}),
    );
    let answers = by_kind(&p, EntryKind::Answer);
    assert_eq!(answers.len(), 2);
    let late = &answers[1];
    assert_eq!(late.placement, Some(Placement::Below));
    assert_eq!(
        late.answers,
        by_kind(&p, EntryKind::Message)[0].id.clone().into()
    );
    assert_eq!(
        late.body_as::<AnswerBody>().unwrap().part.as_deref(),
        Some("answer")
    );
    assert!(out
        .changes
        .iter()
        .any(|ch| ch.entry.id == late.id && ch.entry.rev == 1));
    // the reply to the newest message was in place
    assert_eq!(answers[0].placement, Some(Placement::Inline));
    // the plan closes with its task
    assert_eq!(by_kind(&p, EntryKind::Plan)[0].state, EntryState::Done);
    // the same end told again (its task.result) changes nothing
    let again = p.apply(
        Input::Bus { topic: "task.result", data: &json!({"task_id": "t1", "status": "done", "answer": "3 files.", "request_id": "r1"}) },
        &mut c,
    );
    assert!(again.changes.is_empty());
}

// decision: ORCH-579
#[test]
fn a_stop_told_in_another_reply_is_filed_on_its_own_turn() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "watch the build"}),
    );
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "plan", "task_id": "t1", "text": "Watching.", "request_id": "r1"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "ack", "request_id": "r2", "message": "stop it"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "reply", "text": "Stopped.", "request_id": "r2"}),
    );
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "outcome", "task_id": "t1", "status": "cancelled", "answer": "Stopped.", "told_in": "r2", "request_id": "r1"}),
    );
    let answers = by_kind(&p, EntryKind::Answer);
    assert!(answers
        .iter()
        .all(|a| a.placement == Some(Placement::Inline)));
}

// decision: ORCH-529
#[test]
fn a_merged_message_says_which_message_answers_it_and_an_ended_stream_alone_interrupts() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "weather in Avalon"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "ack", "request_id": "r2", "message": "and Brindle"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "merged", "into": "r1", "request_id": "r2"}),
    );
    p.apply(Input::StreamEnded { request_id: "r2" }, &mut c);
    let m = by_kind(&p, EntryKind::Message);
    assert_eq!(m[1].state, EntryState::Merged);
    assert_eq!(
        m[1].body_as::<MessageBody>()
            .unwrap()
            .merged_into
            .as_deref(),
        Some("r1")
    );
    // r1's stream ends with nothing said and no task: interrupted, with the app's words
    p.apply(Input::StreamEnded { request_id: "r1" }, &mut c);
    let m = by_kind(&p, EntryKind::Message);
    assert_eq!(m[0].state, EntryState::Interrupted);
    assert_eq!(
        m[0].body_as::<MessageBody>().unwrap().notice.as_deref(),
        Some(ferb_supervisor::flow::entries::ENDED_UNCONFIRMED)
    );
}

// decision: ENG-9
#[test]
fn an_error_fails_the_message() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "do it"}),
    );
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "error", "request_id": "r1"}),
    );
    assert_eq!(by_kind(&p, EntryKind::Message)[0].state, EntryState::Failed);
    let e = &by_kind(&p, EntryKind::Error)[0];
    assert_eq!(e.state, EntryState::Failed);
    assert_eq!(
        e.body_as::<AnswerBody>().unwrap().text,
        "FERB could not start this request."
    );
}

/// GATE-604 under ENG-8: the IDs a message's ack says FERB kept sealed are stored on its message entry by kind and
/// saved name (never a value), so a turn rebuilt from entries can say so; a message with none has no `sealed` key, so
/// every body written before reads and writes the same.
// decision: GATE-604
// decision: ENG-8
#[test]
fn the_ids_kept_sealed_from_a_message_are_stored_on_it_by_name() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "my PAN is [PAN]",
               "sealed": [{"kind": "PAN", "name": "PAN 2"}]}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "ack", "request_id": "r2", "message": "hi"}),
    );
    let messages = by_kind(&p, EntryKind::Message);
    assert_eq!(
        messages[0].body["sealed"],
        json!([{"kind": "PAN", "name": "PAN 2"}])
    );
    assert!(
        messages[1].body.get("sealed").is_none(),
        "{}",
        messages[1].body
    );
}

// decision: DMN-583
#[test]
fn a_hidden_inputs_answer_and_a_hidden_secret_are_never_kept() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    for (id, kind) in [
        ("a1", "owner_login_code"),
        ("a2", "owner_question"),
        ("a3", "owner_question"),
    ] {
        p.apply(
            Input::Bus {
                topic: "approval.requested",
                data: &json!({"approval_id": id, "kind": kind, "reason": "Code?"}),
            },
            &mut c,
        );
    }
    let said = |p: &ChatProjector, id: &str| -> Option<String> {
        let q = p
            .entries()
            .into_iter()
            .find(|e| e.approval_id.as_deref() == Some(id))
            .unwrap();
        q.body_as::<QuestionBody>().unwrap().answer.unwrap().said
    };
    p.apply(Input::Bus { topic: "approval.resolved", data: &json!({"approval_id": "a1", "state": "granted", "answer": "words", "said": "123456"}) }, &mut c);
    p.apply(Input::Bus { topic: "approval.resolved", data: &json!({"approval_id": "a2", "state": "granted", "answer": "words", "said": "pw is x", "hidden": ["saved secret"]}) }, &mut c);
    p.apply(Input::Bus { topic: "approval.resolved", data: &json!({"approval_id": "a3", "state": "granted", "answer": "words", "said": "the blue one"}) }, &mut c);
    assert_eq!(said(&p, "a1"), None);
    assert_eq!(said(&p, "a2"), None);
    assert_eq!(said(&p, "a3").as_deref(), Some("the blue one"));
    let q = by_kind(&p, EntryKind::Question);
    assert!(q.iter().all(|e| e.state == EntryState::Answered && !e.open));
}

// decision: ENG-8
#[test]
fn a_card_is_revised_only_when_a_row_really_changes() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "two things"}),
    );
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "plan", "task_id": "t1", "text": "On it.", "request_id": "r1",
        "card": {"request_id": "r1", "rows": [{"part": "p1", "label": "A", "state": "working"}], "done": 0, "total": 1}}),
    );
    let part = json!({"type": "part", "task_id": "t1", "request_id": "r1", "part": "p1", "state": "done", "text": "Done A.",
                      "seq": "2026-01-01T09:00:00.000001+00:00", "done": 1, "total": 1});
    let first = frame(&mut p, &mut c, "r1", part.clone());
    assert_eq!(first.changes.len(), 1);
    // the same change again (the stream and task.part both carry it), and a step's state: nothing stored changes
    assert!(p
        .apply(
            Input::Bus {
                topic: "task.part",
                data: &part
            },
            &mut c
        )
        .changes
        .is_empty());
    let step = frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "step", "task_id": "t1", "step": "worker", "status": "running", "seq": "x"}),
    );
    assert!(step.changes.is_empty());
    assert!(step.live.is_some());
    let plan = &by_kind(&p, EntryKind::Plan)[0];
    assert_eq!(plan.rev, 2);
}

// ---- after a restart (slice 4: the projector rebuilt from stored entries, and the start-up pass) ----

/// Every recorded scenario, its projector rebuilt from the entries it ended with: the rebuilt one holds the same
/// entries, and the same inputs again change nothing (each is made once: a message per request, a question per
/// approval, a notice per id).
// decision: ENG-9
// decision: ENG-13
#[test]
fn a_projector_rebuilt_from_its_entries_holds_them_and_makes_nothing_twice() {
    for sc in scenarios() {
        let r = run(&sc.timeline);
        for (session, entries) in &r.entries {
            let refs: Vec<String> = entries.iter().filter_map(|e| e.reference.clone()).collect();
            let next = entries.iter().map(|e| e.pos).max().unwrap_or(0) + 1;
            let mut p = ChatProjector::restore(session, entries.clone(), refs, next);
            assert_eq!(&p.entries(), entries, "{}: rebuilt", sc.name);
            let mut c = Clock {
                n: 1000,
                seq: 10_000,
            };
            for item in sc
                .timeline
                .iter()
                .filter(|t| s(t, "session").as_deref() == Some(session))
            {
                let again = match item["do"].as_str().unwrap() {
                    "notice" => p.apply(
                        Input::Bus {
                            topic: "attention.notice",
                            data: &item["data"],
                        },
                        &mut c,
                    ),
                    "notification" => p.apply(Input::Notification(&item["item"]), &mut c),
                    "bus" if item["topic"] == "approval.requested" => p.apply(
                        Input::Bus {
                            topic: "approval.requested",
                            data: &item["data"],
                        },
                        &mut c,
                    ),
                    _ => continue,
                };
                assert!(
                    again.changes.iter().all(|ch| ch.entry.rev > 1),
                    "{}: {} made a new entry after the rebuild",
                    sc.name,
                    item["do"]
                );
            }
        }
    }
}

// decision: ENG-9
// decision: ENG-13
#[test]
fn the_start_up_pass_interrupts_cut_work_and_expires_open_questions() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    // a message the restart cut before any task, one whose task won't resume, one whose task will
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "hello?"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "ack", "request_id": "r2", "message": "tidy notes"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "plan", "task_id": "t2", "text": "On it.", "request_id": "r2"}),
    );
    frame(
        &mut p,
        &mut c,
        "r3",
        json!({"type": "ack", "request_id": "r3", "message": "watch it"}),
    );
    frame(
        &mut p,
        &mut c,
        "r3",
        json!({"type": "plan", "task_id": "t3", "text": "Watching.", "request_id": "r3"}),
    );
    p.apply(
        Input::Bus {
            topic: "approval.requested",
            data: &json!({"approval_id": "a1", "kind": "owner_question", "reason": "Which?", "task_id": "t3"}),
        },
        &mut c,
    );
    let stored = p.entries();
    let next = stored.iter().map(|e| e.pos).max().unwrap() + 1;
    let mut back = ChatProjector::restore("s1", stored, Vec::new(), next);
    let changes = back.after_restart(&|t| t == "t3", &mut c);
    assert!(!changes.is_empty());
    fn of(p: &ChatProjector, kind: EntryKind, request: &str) -> Entry {
        p.entries()
            .into_iter()
            .find(|e| e.kind == kind && e.request_id.as_deref() == Some(request))
            .unwrap()
    }
    let m1 = of(&back, EntryKind::Message, "r1");
    assert_eq!(m1.state, EntryState::Interrupted);
    assert_eq!(
        m1.body_as::<MessageBody>().unwrap().notice.as_deref(),
        Some(ferb_supervisor::flow::entries::ENDED_UNCONFIRMED)
    );
    assert_eq!(
        of(&back, EntryKind::Message, "r2").state,
        EntryState::Interrupted
    );
    let plan2 = of(&back, EntryKind::Plan, "r2");
    assert_eq!((plan2.state, plan2.open), (EntryState::Interrupted, false));
    // the resumed task's turn is left as it is
    assert_eq!(of(&back, EntryKind::Message, "r3").state, EntryState::Open);
    assert!(of(&back, EntryKind::Plan, "r3").open);
    let q = back
        .entries()
        .into_iter()
        .find(|e| e.kind == EntryKind::Question)
        .unwrap();
    assert_eq!((q.state, q.open), (EntryState::Expired, false));
    assert_eq!(
        q.body_as::<QuestionBody>().unwrap().answer.unwrap().state,
        "expired"
    );
    // the pass again changes nothing; the resumed task's end still reaches its turn
    assert!(back.after_restart(&|t| t == "t3", &mut c).is_empty());
    let end = back.apply(
        Input::Bus {
            topic: "task.result",
            data: &json!({"task_id": "t3", "status": "done", "answer": "It finished.", "request_id": "r3"}),
        },
        &mut c,
    );
    assert!(end
        .changes
        .iter()
        .any(|ch| ch.entry.kind == EntryKind::Answer));
    assert_eq!(of(&back, EntryKind::Message, "r3").state, EntryState::Done);
}

/// The journal backstop: a reply journaled for a message that has no answer becomes its answer, once.
// decision: ENG-9
// decision: ENG-13
#[test]
fn a_journaled_reply_answers_a_message_left_without_one() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "hi"}),
    );
    let stored = p.entries();
    let mut back = ChatProjector::restore("s1", stored, Vec::new(), 2);
    let made = back.answer_from_journal("r1", "Hello.", &mut c);
    assert!(made.iter().any(|ch| ch.entry.kind == EntryKind::Answer));
    assert!(back.answer_from_journal("r1", "Hello.", &mut c).is_empty());
    assert!(
        back.after_restart(&|_| false, &mut c).is_empty(),
        "answered: nothing to interrupt"
    );
}

// ---- a task two turns ran (APP-919: the live case of 2026-11-15) ----

fn answer_text(e: &Entry) -> String {
    e.body_as::<AnswerBody>()
        .map(|b| b.text)
        .unwrap_or_default()
}

fn follows(e: &Entry) -> Option<String> {
    e.body_as::<AnswerBody>().and_then(|b| b.follows)
}

/// Live 2026-11-15: a stop beside new work, while a task the owner had restarted with a second message ran, showed the
/// stopped task's summary three times (the stop message's plan and receipt, the task's turn, and a later answer on the
/// task's FIRST turn at the bottom). The end is said once on the turn that ran it, and once by the stop message's plan
/// (ORCH-638's receipt); the first turn keeps its own answer and nothing follows it. In both orders the engine can feed
/// the new task's plan and the stopped task's end.
// decision: APP-919
// decision: ORCH-638
// decision: APP-825
#[test]
fn a_stopped_task_two_turns_ran_tells_its_end_once_on_the_turn_that_ran_it() {
    for plan_first in [true, false] {
        let r = run(&stop_beside_new_work_two_turns(plan_first));
        let entries = &r.entries["s1"];
        let of = |kind: EntryKind, request: &str| -> Vec<&Entry> {
            entries
                .iter()
                .filter(|e| e.kind == kind && e.request_id.as_deref() == Some(request))
                .collect()
        };
        let told: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.kind == EntryKind::Answer && answer_text(e) == STOPPED_SUMMARY)
            .collect();
        assert_eq!(
            told.len(),
            1,
            "plan_first {plan_first}: the stopped end is one answer"
        );
        assert_eq!(told[0].request_id.as_deref(), Some("r2"));
        assert_eq!(told[0].placement, Some(Placement::Inline));
        assert_eq!(told[0].state, EntryState::Cancelled);
        assert!(
            entries.iter().all(|e| follows(e).is_none()),
            "no later answer"
        );
        // the first turn keeps the answer its first run ended with, posted below when it came
        let first = of(EntryKind::Answer, "r1");
        assert_eq!(first.len(), 1);
        assert_eq!(
            answer_text(first[0]),
            "Stopped, as you asked.\n\nSites contacted: 1 site"
        );
        assert_eq!(first[0].placement, Some(Placement::Below));
        // the stop message: the stop said first, then the new task's plan line (ORCH-638), and its own task's answer
        let plan: ferb_core::chat_entry::PlanBody = of(EntryKind::Plan, "r3")[0].body_as().unwrap();
        assert_eq!(plan.receipts, Some(vec![STOPPED_SUMMARY.to_string()]));
        assert_eq!(plan.plan_line.as_deref(), Some(FILES_LINE));
        let files = of(EntryKind::Answer, "r3");
        assert_eq!(files.len(), 1);
        assert!(answer_text(files[0]).starts_with("Two files"));
    }
}

/// A history saved before the fix (the stopped end also kept as a later answer on the task's first turn, as the engine
/// stored it that night) is read back whole, and the same end told again makes nothing new.
// decision: APP-919
// decision: ENG-9
#[test]
fn a_history_saved_with_the_extra_answer_is_read_back_and_the_end_again_adds_nothing() {
    let timeline = stop_beside_new_work_two_turns(true);
    let mut stored = run(&timeline).entries["s1"].clone();
    let first = stored
        .iter()
        .find(|e| e.kind == EntryKind::Answer && e.request_id.as_deref() == Some("r1"))
        .cloned()
        .unwrap();
    let mut extra = stored
        .iter()
        .find(|e| e.kind == EntryKind::Answer && answer_text(e) == STOPPED_SUMMARY)
        .cloned()
        .unwrap();
    let mut body: AnswerBody = extra.body_as().unwrap();
    body.follows = Some(first.id.clone());
    extra.id = "old-extra".into();
    extra.request_id = Some("r1".into());
    extra.answers = first.answers.clone();
    extra.pos = stored.iter().map(|e| e.pos).max().unwrap() + 1;
    extra.body = ferb_core::chat_entry::body(&body);
    stored.push(extra);
    let next = stored.iter().map(|e| e.pos).max().unwrap() + 1;
    let mut back = ChatProjector::restore("s1", stored.clone(), Vec::new(), next);
    assert_eq!(back.entries(), stored, "read back whole");
    let mut c = Clock {
        n: 1000,
        seq: 10_000,
    };
    for item in timeline
        .iter()
        .filter(|t| t["do"] == "bus" && t["data"]["task_id"] == "t-assign")
    {
        let again = back.apply(
            Input::Bus {
                topic: "task.result",
                data: &item["data"],
            },
            &mut c,
        );
        assert!(
            again.changes.iter().all(|ch| ch.entry.rev > 1),
            "nothing new"
        );
    }
}

/// A result read back with no request and no session (after a restart, `chat.status`) ends the task's newest turn,
/// never an earlier one it ran under: that turn already has its answer.
// decision: APP-919
#[test]
fn a_result_with_no_request_ends_the_newest_turn_of_its_task() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    for (r, text) in [("r1", "watch the build"), ("r2", "start it again")] {
        frame(
            &mut p,
            &mut c,
            r,
            json!({"type": "ack", "request_id": r, "message": text}),
        );
        if r == "r1" {
            frame(
                &mut p,
                &mut c,
                "r1",
                json!({"type": "plan", "task_id": "t1", "text": "Watching.", "request_id": "r1"}),
            );
            frame(
                &mut p,
                &mut c,
                "r1",
                json!({"type": "outcome", "task_id": "t1", "status": "cancelled", "answer": "Stopped.", "request_id": "r1"}),
            );
        }
    }
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "plan", "task_id": "t1", "text": "Watching again.", "request_id": "r2"}),
    );
    let next = p.next_pos();
    let mut back = ChatProjector::restore("s1", p.entries(), Vec::new(), next);
    back.apply(
        Input::Bus {
            topic: "task.result",
            data: &json!({"task_id": "t1", "status": "done", "answer": "The build passed."}),
        },
        &mut c,
    );
    let answers = by_kind(&back, EntryKind::Answer);
    let ended: Vec<&Entry> = answers
        .iter()
        .filter(|e| answer_text(e) == "The build passed.")
        .collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].request_id.as_deref(), Some("r2"));
    assert!(follows(ended[0]).is_none());
}

// ---- the order rules over invented interleavings (journal 72's open item) ----

/// A small deterministic generator (xorshift64*): the same seed, the same interleaving.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n
    }
}

/// A timeline's lanes: each stream's items (its send, frames and end, with the `task.result` chat.rs publishes right
/// after an outcome), and everything else (questions, answers, results and notices `chat.status` read, banners), each
/// lane in its own order.
fn lanes(timeline: &[Value]) -> Vec<Vec<Value>> {
    let mut streams: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    let mut other = Vec::new();
    let mut last_outcome: Option<(u64, Option<String>)> = None;
    for item in timeline {
        if let Some(n) = item.get("stream").and_then(Value::as_u64) {
            last_outcome =
                (item["frame"]["type"] == "outcome").then(|| (n, s(&item["frame"], "task_id")));
            streams.entry(n).or_default().push(item.clone());
            continue;
        }
        if item["do"] == "bus" && item["topic"] == "task.result" {
            if let Some((n, task)) = last_outcome.take() {
                if s(&item["data"], "task_id") == task {
                    streams.entry(n).or_default().push(item.clone());
                    continue;
                }
            }
        }
        last_outcome = None;
        other.push(item.clone());
    }
    let mut out: Vec<Vec<Value>> = streams.into_values().collect();
    if !other.is_empty() {
        out.push(other);
    }
    out
}

/// What an item needs to have happened before it, by what it names: a request's message (its send), a task's plan,
/// a question's asking (an answer to it). Causes an interleaving keeps, as the engine's one queue does.
fn needs(item: &Value) -> (Vec<String>, Vec<String>, Vec<String>) {
    let data = if item["do"] == "frame" {
        &item["frame"]
    } else {
        &item["data"]
    };
    let mut requests: Vec<String> = s(data, "request_id").into_iter().collect();
    requests.extend(s(data, "into"));
    let task = s(data, "task_id").map(|t| t.split('.').next().unwrap_or_default().to_string());
    let mut asked = Vec::new();
    if item["do"] == "bus" && item["topic"] == "approval.resolved" {
        asked.extend(s(data, "approval_id"));
    }
    let tasks = if item["do"] == "frame" && data["type"] == "plan" {
        Vec::new()
    } else {
        task.into_iter().collect()
    };
    (requests, tasks, asked)
}

/// What an item makes happen for others: the request it sends, the task it plans, the question it asks.
fn gives(item: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let request = (item["do"] == "send")
        .then(|| s(item, "request_id"))
        .flatten();
    let plan = (item["do"] == "frame" && item["frame"]["type"] == "plan")
        .then(|| s(&item["frame"], "task_id"))
        .flatten();
    let asked = if item["do"] == "frame" && item["frame"]["topic"] == "approval.requested" {
        s(&item["frame"]["data"], "approval_id")
    } else if item["do"] == "bus" && item["topic"] == "approval.requested" {
        s(&item["data"], "approval_id")
    } else {
        None
    };
    (request, plan, asked)
}

/// One random interleaving of the lanes that keeps each lane's order and every item's causes (`needs`) before it.
fn interleave(lanes: &[Vec<Value>], rng: &mut Rng) -> Vec<Value> {
    let mut at = vec![0usize; lanes.len()];
    let all: Vec<&Value> = lanes.iter().flatten().collect();
    let (mut sent, mut planned, mut asked) = (HashSet::new(), HashSet::new(), HashSet::new());
    let exists = |pick: &dyn Fn(&Value) -> Option<String>, x: &str| {
        all.iter().any(|i| pick(i).as_deref() == Some(x))
    };
    let mut out = Vec::new();
    loop {
        let ready: Vec<usize> = (0..lanes.len())
            .filter(|l| at[*l] < lanes[*l].len())
            .filter(|l| {
                let (requests, tasks, questions) = needs(&lanes[*l][at[*l]]);
                requests
                    .iter()
                    .all(|r| sent.contains(r) || !exists(&|i| gives(i).0, r))
                    && tasks
                        .iter()
                        .all(|t| planned.contains(t) || !exists(&|i| gives(i).1, t))
                    && questions
                        .iter()
                        .all(|a| asked.contains(a) || !exists(&|i| gives(i).2, a))
            })
            .collect();
        if ready.is_empty() {
            // nothing left, or the rest wait on each other (the recorded order is kept for them)
            for (l, lane) in lanes.iter().enumerate() {
                out.extend(lane[at[l]..].iter().cloned());
            }
            return out;
        }
        let l = ready[rng.below(ready.len())];
        let item = lanes[l][at[l]].clone();
        at[l] += 1;
        let (r, p, a) = gives(&item);
        sent.extend(r);
        planned.extend(p);
        asked.extend(a);
        out.push(item);
    }
}

/// The order rules (`support_entries`) on the pure projector over every recorded scenario: in 40 invented
/// interleavings of its lanes each, or as recorded when it has one lane (two streams at once, a question or a result beside a stream): whatever the
/// interleaving, a message comes before its plan, answers, questions and error, a plan before its task's answers and
/// questions, a question asked before its task's end before that end, an answer after what it answers, and catching up
/// from any cut gives exactly the stored entries above it.
// decision: ENG-8
// decision: ENG-13
// decision: ENG-15
#[test]
fn the_order_rules_hold_over_invented_interleavings() {
    let mut runs = 0;
    let mut failures = Vec::new();
    for sc in scenarios() {
        let lanes = lanes(&sc.timeline);
        // one lane: its recorded order is the only one. Several: the recorded timeline itself is not an order the
        // engine can make (it puts a stream's frames at the stream's end, after a question `chat.status` read that
        // the stream's own plan preceded: README "Where the timelines come from"), so only interleavings that keep
        // every cause before its effect
        let mut timelines = Vec::new();
        if lanes.len() <= 1 {
            timelines.push(sc.timeline.clone());
        } else {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (sc.name.len() as u64 + 1));
            for _ in 0..40 {
                timelines.push(interleave(&lanes, &mut rng));
            }
        }
        for (n, timeline) in timelines.iter().enumerate() {
            let r = run(timeline);
            runs += 1;
            let items: Vec<Value> = r
                .made
                .iter()
                .map(|(session, item, out)| {
                    json!({"conversation": session, "input": item,
                           "changes": out.changes.iter().map(|c| &c.entry).collect::<Vec<_>>()})
                })
                .collect();
            let stored: Vec<Entry> = r.entries.values().flatten().cloned().collect();
            let trace = support_entries::read(&items);
            for line in support_entries::check(&format!("{} #{n}", sc.name), &trace, &stored) {
                failures.push(line);
            }
        }
    }
    assert!(runs > 71 * 10, "interleavings were made ({runs} runs)");
    eprintln!("{runs} timelines checked");
    assert!(
        failures.is_empty(),
        "{} broken:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The rules see a broken order: an answer moved above its message, a plan made after its task's question, a revision
/// the history lost (catch-up), two entries at one position.
// decision: ENG-8
// decision: ENG-15
#[test]
fn the_order_rules_see_a_broken_order() {
    let all = scenarios();
    let sc = all.iter().find(|s| s.name == "confirm_yes").unwrap();
    let mut rng = Rng(7);
    let r = run(&interleave(&lanes(&sc.timeline), &mut rng));
    let items: Vec<Value> = r
        .made
        .iter()
        .map(|(session, item, out)| {
            json!({"conversation": session, "input": item,
                   "changes": out.changes.iter().map(|c| &c.entry).collect::<Vec<_>>()})
        })
        .collect();
    let stored: Vec<Entry> = r.entries.values().flatten().cloned().collect();
    let check = |items: &[Value], stored: &[Entry]| {
        support_entries::check("t", &support_entries::read(items), stored)
    };
    assert!(check(&items, &stored).is_empty());
    let of = |kind: EntryKind| stored.iter().position(|e| e.kind == kind).unwrap();
    // swapped positions of a message and its answer, everywhere they appear
    let (m, a) = (
        stored[of(EntryKind::Message)].clone(),
        stored[of(EntryKind::Answer)].clone(),
    );
    let swap = |v: &mut Value| {
        if v["id"] == json!(m.id) {
            v["pos"] = json!(a.pos);
        } else if v["id"] == json!(a.id) {
            v["pos"] = json!(m.pos);
        }
    };
    let mut moved_items = items.clone();
    for i in &mut moved_items {
        for c in i["changes"].as_array_mut().unwrap() {
            swap(c);
        }
    }
    let mut moved: Vec<Entry> = stored.clone();
    for e in &mut moved {
        let mut v = serde_json::to_value(&*e).unwrap();
        swap(&mut v);
        *e = serde_json::from_value(v).unwrap();
    }
    let broken = check(&moved_items, &moved).join("\n");
    assert!(
        broken.contains("(rule 3)") && broken.contains("(rule 6)"),
        "{broken}"
    );
    // the plan made after the question its task's worker asked
    let (p, q) = (
        stored[of(EntryKind::Plan)].clone(),
        stored[of(EntryKind::Question)].clone(),
    );
    let mut late: Vec<Entry> = stored.clone();
    for e in &mut late {
        if e.id == p.id {
            e.pos = q.pos;
        } else if e.id == q.id {
            e.pos = p.pos;
        }
    }
    assert!(check(&items, &late).join("\n").contains("(rule 4)"));
    // a revision the history lost: catching up from before it misses it
    let mut lost = items.clone();
    let last = lost
        .iter_mut()
        .rev()
        .find(|i| {
            i["changes"]
                .as_array()
                .is_some_and(|c| c.iter().any(|e| e["rev"] != 1))
        })
        .unwrap();
    last["changes"] = json!([]);
    assert!(check(&lost, &stored).join("\n").contains("(rule 7)"));
    // two entries at one position
    let mut twice = stored.clone();
    twice[1].pos = twice[0].pos;
    assert!(check(&items, &twice)
        .join("\n")
        .contains("two entries at pos"));
}

/// Every entry of a message's turn names the client that sent it, a revision after a rebuild included; a message sent
/// with no client id names none (ENG-15: a client tells its own turns from the others by any of their entries).
// decision: ENG-15
#[test]
fn every_entry_of_a_turn_names_the_client_that_sent_it() {
    let (mut p, mut c) = (ChatProjector::new("s1", 1), Clock { n: 0, seq: 0 });
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "ack", "request_id": "r1", "message": "tidy the folder",
               "client_id": "phone-1", "submission_id": "sub-1"}),
    );
    frame(
        &mut p,
        &mut c,
        "r1",
        json!({"type": "plan", "request_id": "r1", "task_id": "t1", "text": "Tidying."}),
    );
    p.apply(
        Input::Bus {
            topic: "approval.requested",
            data: &json!({"approval_id": "ap1", "request_id": "r1", "task_id": "t1.worker",
                          "kind": "owner_question", "reason": "Which folder?", "options": ["Desktop"]}),
        },
        &mut c,
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "ack", "request_id": "r2", "message": "and the weather?"}),
    );
    frame(
        &mut p,
        &mut c,
        "r2",
        json!({"type": "reply", "request_id": "r2", "text": "Sunny."}),
    );
    let stored = p.entries();
    for e in &stored {
        let want = (e.request_id.as_deref() == Some("r1")).then_some("phone-1");
        assert_eq!(
            e.client_id.as_deref(),
            want,
            "{:?} of {:?}",
            e.kind,
            e.request_id
        );
    }
    assert_eq!(
        stored.iter().filter(|e| e.client_id.is_some()).count(),
        3,
        "message, plan, question"
    );
    // rebuilt after a restart: the task's end still names the client
    let mut back = ChatProjector::restore("s1", stored, Vec::new(), 6);
    let end = back.apply(
        Input::Bus {
            topic: "task.result",
            data: &json!({"type": "outcome", "task_id": "t1", "request_id": "r1", "status": "done",
                          "answer": "Tidied.", "verified": true}),
        },
        &mut c,
    );
    let answer = end
        .changes
        .iter()
        .find(|ch| ch.entry.kind == EntryKind::Answer)
        .expect("the task's end");
    assert_eq!(answer.entry.client_id.as_deref(), Some("phone-1"));
}
