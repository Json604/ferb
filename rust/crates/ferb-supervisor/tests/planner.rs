//! The planner's context, call by call, against the 38 planner calls of the 28 daemon scenarios
//! (rust/fixtures/supervisor/scenarios.json, recorded at supervisor.py's composition points while
//! rust/fixtures/daemon/make.py's scenarios ran): the conversation from the journal rows it read, the open
//! questions, the latest result, the previous scheduled run, the block list `_supervise` builds, Tier 1's newly
//! always-on entries, the tasks block from the trees' state as `_task_state` read it, and the budgeted parts the model
//! received; and each turn's Layer 5 payload from the messages as sent (`_take_in`).
//!
//! What happened since (`_since_block`) is none in these scenarios: its recorded output (None) stands in here, and
//! units.json checks the builder itself.

mod common;

use std::collections::BTreeSet;

use common::*;
use ferb_clock::py::iso;
use ferb_clock::zone::Zone;
use ferb_context::tokens::Tokenizer;
use ferb_supervisor::history::{
    conversation_blocks, latest_result, timeline, Entry, Sourced, Timeline, TimelineReads,
};
use ferb_supervisor::planner::*;
use ferb_supervisor::seams::JournalRow;
use ferb_supervisor::words::FERB_CHECKS;
use serde_json::{json, Map, Value};

struct Reads<'a>(&'a Map<String, Value>);

impl TimelineReads for Reads<'_> {
    fn outcome_json(&self, task_id: &str) -> Option<String> {
        self.0
            .get(task_id)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
    fn notice_sources(&self, _notice_id: &str) -> Vec<String> {
        panic!("no notice in these scenarios")
    }
}

fn rows(v: &Value) -> Vec<JournalRow> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| JournalRow::from_json(r).expect("a row"))
        .collect()
}

fn entry_json(e: &Entry) -> Value {
    json!({"at": iso(&e.at), "text": e.text, "payload": e.payload, "reference": e.reference, "chat": e.chat,
           "origins": e.origins, "handle": e.handle, "kind": e.kind, "refs": e.refs, "opens": e.opens,
           "values": e.values, "result": e.result})
}

fn same(got: &[Sourced], want: &[Sourced], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: how many blocks");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!(g, w, "{what}: block {i}");
    }
}

fn tree_view(t: &Value) -> TreeView {
    TreeView {
        task_id: s(&t["task_id"]),
        mine: t["mine"].as_bool().unwrap(),
        schedule: t["schedule"].as_bool().unwrap(),
        goal: s(&t["goal"]),
        stopping: t["stop"].as_bool().unwrap(),
        reports: t["reports"].as_u64().unwrap() as usize,
        last_report: t["last_report"].as_object().cloned(),
        steps: t["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (s(&p[0]), s(&p[1])))
            .collect(),
        loops: t["loops"].as_array().cloned().unwrap_or_default(),
        paused: t["paused"].as_bool().unwrap(),
        question: s(&t["question"]),
        cards: t["cards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_object().cloned().unwrap())
            .collect(),
        last_step: (!t["last_step"].is_null()).then(|| t["last_step"].clone()),
        asleep: t["asleep"].as_object().cloned(),
        preview: t["preview"].as_array().map(|p| (s(&p[0]), s(&p[1]))),
        goals: if t["plan"].as_bool().unwrap() {
            t["goals"].as_object().cloned()
        } else {
            None
        },
        runtime: s(&t["runtime"]),
        standing: t["standing"].as_array().cloned().unwrap_or_default(),
        folders: strs(&t["folders"]),
        origins: set(&t["origins"]),
    }
}

/// One planner call: the records from its `supervise` up to its `assemble`.
struct Call<'a> {
    supervise: &'a Value,
    group: Vec<&'a Value>,
}

impl<'a> Call<'a> {
    fn one(&self, kind: &str) -> &'a Value {
        let found: Vec<&&Value> = self.group.iter().filter(|r| r["kind"] == kind).collect();
        assert_eq!(found.len(), 1, "one {kind} per planner call");
        found[0]
    }
}

fn calls(records: &[Value]) -> Vec<Call<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < records.len() {
        if records[i]["kind"] == "supervise" {
            let mut group = Vec::new();
            let mut j = i + 1;
            while j < records.len() && records[j]["kind"] != "assemble" {
                group.push(&records[j]);
                j += 1;
            }
            group.push(&records[j]);
            out.push(Call {
                supervise: &records[i],
                group,
            });
            i = j;
        }
        i += 1;
    }
    out
}

#[test]
fn every_planner_call() {
    let zone = Zone::utc();
    let tokens = Tokenizer::Utf8Bytes;
    let root = root();
    let mut checked = 0;
    let mut timelines = 0;
    let mut task_blocks = 0;
    for (name, records) in scenarios() {
        for call in calls(records) {
            let sup = call.supervise;
            let what = format!("{name} {}", s(&sup["request_id"]));
            // the conversation, from the journal rows `_timeline` read
            let tl = call.one("timeline");
            let current = set(&tl["current"]);
            let rws = rows(&tl["rows"]);
            let task_ids = tl["task_ids"].as_array().map(|_| set(&tl["task_ids"]));
            let covered = set(&tl["covered"]);
            let outcomes = tl["outcomes"].as_object().cloned().unwrap_or_default();
            let (entries, ages) = timeline(&Timeline {
                current: &current,
                conversation: tl["conversation"].as_str(),
                rows: &rws,
                task_ids: task_ids.as_ref(),
                covered: &covered,
                reads: &Reads(&outcomes),
                zone: &zone,
            });
            let got: Vec<Value> = entries.iter().map(entry_json).collect();
            assert_eq!(Value::Array(got), tl["entries"], "{what}: the timeline");
            let got_ages: Map<String, Value> = ages
                .iter()
                .map(|(k, t)| (k.clone(), json!(iso(t))))
                .collect();
            assert_eq!(Value::Object(got_ages), tl["ages"], "{what}: the ages");
            timelines += 1;
            let cb = call.one("conversation_blocks");
            let summary = cb["summary"]
                .as_object()
                .map(|_| JournalRow::from_json(&cb["summary"]).unwrap());
            let latest = latest_result(&entries);
            assert_eq!(latest, s(&cb["latest"]), "{what}: the latest result");
            let conversation = conversation_blocks(entries, summary.as_ref(), false, tokens);
            let want: Vec<Sourced> = cb["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(sourced)
                .collect();
            same(&conversation, &want, &format!("{what}: conversation"));
            // the open questions
            let qb = call.one("questions_block");
            let pending: Vec<OpenQuestion> = qb["pending"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| OpenQuestion {
                    id: s(&a["id"]),
                    kind: s(&a["kind"]),
                    capability: s(&a["capability"]),
                    reason: s(&a["reason"]),
                    options: strs(&a["options"]),
                    task_label: s(&a["task_label"]),
                    card_row: a["card_row"].as_object().cloned().unwrap(),
                })
                .collect();
            let questions = questions_block(
                &pending,
                qb["conversations"].as_array().unwrap(),
                &FERB_CHECKS,
                &wrap,
            );
            assert_eq!(questions, sourced(&qb["out"]), "{what}: questions");
            // the latest result
            let lb = call.group.iter().find(|r| r["kind"] == "latest_blocks");
            let latest_got = match lb {
                Some(lb) => {
                    let found = rows(&lb["found"]);
                    let parts = rows(&lb["part_rows"]);
                    let order = strs(&lb["part_order"]);
                    let got = latest_blocks(
                        &LatestReads {
                            found: found.first(),
                            part_rows: &parts,
                            part_order: &order,
                        },
                        tokens,
                        &zone,
                    );
                    let want: Vec<Sourced> =
                        lb["out"].as_array().unwrap().iter().map(sourced).collect();
                    same(&got, &want, &format!("{what}: latest"));
                    got
                }
                None => Vec::new(),
            };
            // the tasks block, from the trees' state as `_task_state` read it
            let ts = call.one("task_state");
            let trees: Vec<TreeView> = ts["trees"]
                .as_array()
                .unwrap()
                .iter()
                .map(tree_view)
                .collect();
            let waiting: Vec<WaitingTask> = ts["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| WaitingTask {
                    task_id: s(&c["task_id"]),
                    goal: s(&c["goal"]),
                    status: s(&c["status"]),
                    prior_task: c["prior_task"].as_str().map(str::to_string),
                    origins: set(&c["origins"]),
                })
                .collect();
            let tasks = task_state(&trees, &waiting, tokens, &wrap);
            assert_eq!(
                tasks,
                ts["out"].as_object().map(|_| sourced(&ts["out"])),
                "{what}: the tasks block"
            );
            if tasks.is_some() {
                task_blocks += 1;
            }
            let since = call
                .group
                .iter()
                .find(|r| r["kind"] == "since_block")
                .and_then(|r| r["out"].as_object().map(|_| sourced(&r["out"])));
            assert!(
                !sup["screen"].as_bool().unwrap() && sup["screens"] == 0,
                "{what}: no front window recorded"
            );
            assert_eq!(sup["last_call"], "", "{what}");
            let session = s(&sup["session"]);
            let previous = if !sup["schedule_previous"].is_null() {
                panic!("{what}: a previous run's block isn't in these scenarios")
            } else if session.starts_with("schedule:") {
                Some(previous_run_block(None, &zone))
            } else {
                None
            };
            let open = sup["open"].as_object().map(|o| OpenAsk {
                asked: s(&o["asked"]),
                payload: s(&o["payload"]),
                carried: o["carried"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| (s(&e["id"]), s(&e["origin"])))
                    .collect(),
                asked_outside: set(&o["asked_outside"]),
            });
            let repair = sup["repair"].as_array().map(|_| strs(&sup["repair"]));
            let repair_md = root.load("orchestrator/prompts/repair.md", &[]).unwrap();
            let now = ferb_clock::py::parse(&s(&sup["now"])).unwrap();
            let entry_ids: Vec<String> = sup["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| s(&e["id"]))
                .collect();
            let last_runs: BTreeSet<String> = set(&sup["last_run_origins"]);
            let message_origin = set(&sup["message_origin"]);
            let (all, first, last) = planner_sourced(PlannerTurn {
                session: &session,
                preview: sup["preview"].as_bool().unwrap(),
                stable: &s(&sup["environment"]["stable"]),
                live_env: &s(&sup["environment"]["live"]),
                live_facts: &s(&sup["live_facts"]),
                last_run_origins: &last_runs,
                now: &now,
                owner_active: None,
                last_event: None,
                conversation,
                ages: &ages,
                tasks,
                since,
                latest: latest_got,
                windows: Vec::new(),
                previous_run: previous,
                questions: Some(questions),
                repair: repair.as_ref().map(|r| (repair_md.as_str(), r.as_slice())),
                last_call: None,
                open: open.as_ref(),
                entries: &entry_ids,
                payload: &s(&sup["payload"]),
                message_origin: &message_origin,
                wrap: &wrap,
                zone: &zone,
            });
            assert!(sup["owner_active"].is_null() && sup["last_event"].is_null());
            let ctx = planner_context(&all, first, last);
            let newly = call.one("newly_always_on");
            let ctx = with_newly_always_on(ctx, &s(&newly["pending"]));
            let asm = call.one("assemble");
            let want_blocks: Vec<_> = asm["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(block)
                .collect();
            assert_eq!(
                ctx.len(),
                want_blocks.len(),
                "{what}: how many context blocks"
            );
            for (i, (g, w)) in ctx.iter().zip(&want_blocks).enumerate() {
                assert_eq!(g, w, "{what}: context block {i}");
            }
            let assembly = assemble_run(
                ctx,
                &[],
                asm["budget_tokens"].as_i64().unwrap(),
                asm["reserved_tokens"].as_i64().unwrap(),
                asm["max_chars"].as_i64(),
                tokens,
            )
            .unwrap();
            assert_eq!(
                assembly.parts,
                strs(&asm["parts"]),
                "{what}: the parts the model received"
            );
            assert_eq!(
                assembly.cacheable as u64,
                asm["cacheable"].as_u64().unwrap(),
                "{what}: cacheable"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 38, "planner calls");
    assert_eq!(task_blocks, 5, "planner calls with a tasks block");
    assert_eq!(timelines, 38);
}

/// The owner's messages each chat turn read, as sent: request id -> (text, input_paths), from the daemon
/// fixture's `chat.send` lines and the `ack` that named their request.
fn sent_messages(scenario: &Value) -> std::collections::HashMap<String, (String, Vec<String>)> {
    let mut pending: std::collections::HashMap<String, Vec<(String, Vec<String>)>> =
        Default::default();
    let mut out = std::collections::HashMap::new();
    for step in scenario["steps"].as_array().unwrap() {
        let conn = s(&step["conn"]);
        for line in step["lines"].as_array().into_iter().flatten() {
            if let Some(sent) = line["send"].as_str() {
                let msg: Value = serde_json::from_str(&s(&resolve(&Value::String(sent.into()))))
                    .unwrap_or(Value::Null);
                if msg["method"] == "chat.send" {
                    let p = &msg["params"];
                    pending
                        .entry(conn.clone())
                        .or_default()
                        .push((s(&p["text"]), strs(&p["input_paths"])));
                }
            } else if let Some(got) = line["recv"].as_str() {
                let msg: Value = serde_json::from_str(got).unwrap_or(Value::Null);
                if msg["params"]["type"] == "ack" {
                    let queue = pending.get_mut(&conn).expect("an ack follows a send");
                    out.insert(s(&msg["params"]["request_id"]), queue.remove(0));
                }
            }
        }
    }
    out
}

/// supervisor.py:2575 `_take_in`: each chat turn's payload (the Layer 5 message the planner read) from the messages
/// as sent, their journaled attachments and FERB's open question.
#[test]
fn every_turn_payload() {
    use ferb_supervisor::intake::{take_in, Arrival, Start};
    let daemon = read("rust/fixtures/daemon/chat_scenarios.json");
    let mut checked = 0;
    for (name, records) in scenarios() {
        let sc = daemon["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sc| sc["name"] == name.as_str())
            .unwrap();
        let sent = sent_messages(sc);
        for sup in records.iter().filter(|r| r["kind"] == "supervise") {
            if s(&sup["session"]).starts_with("schedule:") {
                continue; // a scheduled run's request is its schedule's instructions (run_scheduled, not ported)
            }
            let arrivals: Vec<Arrival> = sup["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    let rid = s(&e["data"]["request_id"]);
                    let (text, raw_paths) = sent
                        .get(&rid)
                        .unwrap_or_else(|| panic!("{name}: no message {rid}"))
                        .clone();
                    Arrival {
                        marked: ferb_sanitize::untrusted::mark(&text),
                        paths: strs(&e["data"]["input_paths"]),
                        attached: !raw_paths.is_empty(),
                        hidden: Vec::new(),
                    }
                })
                .collect();
            let open = &sup["open"];
            let start = Start {
                asked: s(&open["asked"]),
                payload: s(&open["payload"]),
                ..Default::default()
            };
            let turn = take_in(&arrivals, &start, &wrap);
            assert_eq!(
                turn.payload,
                s(&sup["payload"]),
                "{name} {}",
                s(&sup["request_id"])
            );
            for (a, e) in arrivals.iter().zip(sup["entries"].as_array().unwrap()) {
                assert_eq!(
                    a.marked.for_model,
                    s(&e["data"]["text"]),
                    "{name}: the journaled message"
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 37);
}
