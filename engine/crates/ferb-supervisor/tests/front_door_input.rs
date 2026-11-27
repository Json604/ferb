//! Front door step 8's numbers (kb/plans/front-door.md "Targets": 8k tokens fixed and 15k on an ordinary turn are the
//! agent's estimate there, a measurement, never a target forced at the cost of quality): what the front door and the
//! planner are sent on recorded chat scenarios, block by block (characters and FERB's token estimate,
//! `ferb_context::tokens::count_tokens`), printed for the journal; and guards on the numbers reached (with headroom).
//!
//! The fixed part is what every front-door call carries before the conversation: its instructions (the shared rules,
//! concierge.md and the facts about this FERB), the card at its whole budget (1,500 tokens, ORCH-719: the recordings
//! hold a Python-era profile, so the card's limit stands in for it), the tool descriptions it is offered, Layer 1 and
//! the library at its size on an invented five-year store in November (14 days of 6 lines, 10 months, 5 years, the
//! contents). The ordinary turn is the recorded plain reply after 20 invented exchanges of everyday length. Through
//! the real supervisor with stand-in models (temp store, injected ids and clock, no provider).

mod common;
mod support_control;

use std::sync::atomic::Ordering;

use ferb_context::tokens::count_tokens;
use serde_json::{json, Map, Value};
use support_control::{run_with, world_with, World};

/// ORCH-719 (owner): the card's limit.
const CARD_TOKENS: usize = 1_500;

fn given(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::Array(a) => !a.is_empty(),
        _ => true,
    }
}

/// The recorded planner answer as the front door's calls (front_door.rs's conversion).
fn convert(answer: &Value) -> Vec<Value> {
    let side = &answer["side_entries"];
    let aq = answer["answers_question"].as_bool().unwrap_or(false);
    let mut events = Vec::new();
    let mut part = |tool: &str, fields: &[(&str, &Value)]| {
        let mut args = Map::new();
        for (k, v) in fields {
            if given(v) {
                args.insert(k.to_string(), (*v).clone());
            }
        }
        if !args.is_empty() {
            args.insert("answers_question".into(), json!(aq));
            events.push(json!({"kind": "front_call", "name": tool, "arguments": args}));
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
    let text = answer["text"].as_str().unwrap_or_default().to_string();
    match answer["kind"].as_str() {
        Some("task") => events.push(json!({"kind": "front_call", "name": "chat.dispatch_task",
            "arguments": {"ack": text, "goal": answer["task"]["goal"], "answers_question": aq}})),
        Some("clarify") => events.push(
            json!({"kind": "front_call", "name": "chat.ask", "arguments": {"question": text}}),
        ),
        _ => events.push(json!({"kind": "text_delta", "data": {"text": text}})),
    }
    events.push(json!({"kind": "usage", "data": {"input_tokens": 10, "output_tokens": 5}}));
    let final_text = if answer["kind"] == "reply" {
        text
    } else {
        String::new()
    };
    events.push(json!({"kind": "final", "data": {"text": final_text}}));
    events
}

fn recorded_answer(call: &Value) -> Value {
    let text = call["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "final")
        .map(|e| e["data"]["text"].as_str().unwrap().to_string())
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The scenario through the front door (each recorded planner answer as its calls; a hand-off's recorded planner call
/// after it), with `seed` run on the store first.
fn front(name: &str, seed: impl FnOnce(&World)) -> World {
    let handing: std::sync::Arc<std::sync::Mutex<Vec<bool>>> = Default::default();
    let flags = handing.clone();
    let mut w = world_with(name, move |calls| {
        let mut out = Vec::new();
        for c in calls.drain(..) {
            if c["key"] != "supervisor" {
                out.push(c);
                continue;
            }
            let answer = recorded_answer(&c);
            let mut f = c.clone();
            f["events"] = Value::Array(convert(&answer));
            let hands_off = answer["kind"] == "task";
            out.push(f);
            flags.lock().unwrap().push(hands_off);
            if hands_off {
                out.push(c);
            }
        }
        *calls = out;
    });
    w.daemon.bounded.store(true, Ordering::SeqCst);
    w.daemon.front.store(true, Ordering::SeqCst);
    {
        let handing = handing.lock().unwrap().clone();
        let mut q = w.daemon.supervise.lock().unwrap();
        let mut at = 0;
        for hands_off in handing {
            if at >= q.len() {
                break;
            }
            if hands_off {
                let rec = q[at].clone();
                q.insert(at + 1, rec);
                at += 1;
            }
            at += 1;
        }
    }
    seed(&w);
    let _ = run_with(&mut w, name, |_| {});
    w.sup.chat_flush();
    w
}

/// Each orchestrator call: (whether it was offered the front door's tools, instructions, tools offered, parts).
fn calls(w: &World) -> Vec<(bool, String, Vec<String>, Vec<String>)> {
    let calls = w.daemon.calls.lock().unwrap();
    calls
        .started
        .iter()
        .filter_map(|c| {
            let n = c["n"].as_u64()? as usize;
            let made = &calls.made[n];
            if made["runtime_role"] != "orchestrator" {
                return None;
            }
            let tools: Vec<String> = made["tools"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let front = tools.iter().any(|t| t == "chat.dispatch_task");
            let parts = c["parts"]
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect();
            Some((
                front,
                c["system"].as_str().unwrap_or_default().to_string(),
                tools,
                parts,
            ))
        })
        .collect()
}

/// The descriptions of the tools named, as the gateway lists them (the frozen catalog and the engine's additions).
fn tool_rows(names: &[String]) -> String {
    let root = common::repo();
    let read = |p: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(p)).unwrap()).unwrap()
    };
    let catalog = read("engine/fixtures/catalog/tools.json");
    let additions = read("engine/crates/ferb-gateway/src/catalog_additions.json");
    let mut all: Vec<Value> = catalog["tools"].as_array().unwrap().clone();
    if let Some(a) = additions["tools"].as_array() {
        all.extend(
            a.iter()
                .map(|t| t.get("tool").cloned().unwrap_or_else(|| t.clone())),
        );
    }
    let rows: Vec<Value> = names
        .iter()
        .filter_map(|n| {
            all.iter()
                .find(|t| t["name"] == n.as_str())
                .map(|t| json!({"name": t["name"], "description": t["description"], "input_schema": t["input_schema"]}))
        })
        .collect();
    ferb_py::json::dumps(&Value::Array(rows))
}

fn block_name(part: &str) -> &'static str {
    if part.starts_with("Layer 1 — the library") {
        "library"
    } else if part.starts_with("Layer 1") {
        "layer 1"
    } else if part.starts_with("Layer 5 — conversation:") {
        "conversation header"
    } else if part.starts_with("[c") {
        "conversation entries"
    } else {
        "tail"
    }
}

/// One call's blocks: (name, chars, tokens), the instructions and tools first.
fn blocks(system: &str, tools: &[String], parts: &[String]) -> Vec<(String, usize, usize)> {
    let mut out: Vec<(String, usize, usize)> = Vec::new();
    let mut add = |name: &str, text: &str| match out.iter_mut().find(|(n, _, _)| n == name) {
        Some(row) => {
            row.1 += text.chars().count();
            row.2 += count_tokens(text);
        }
        None => out.push((name.to_string(), text.chars().count(), count_tokens(text))),
    };
    add("instructions", system);
    add("tools", &tool_rows(tools));
    for p in parts {
        add(block_name(p), p);
    }
    out
}

fn table(label: &str, rows: &[(String, usize, usize)]) -> String {
    let total: (usize, usize) = rows.iter().fold((0, 0), |a, r| (a.0 + r.1, a.1 + r.2));
    let mut s = format!("{label}: {} chars, {} tokens", total.0, total.1);
    for (n, c, t) in rows {
        s.push_str(&format!("\n    {n}: {c} chars, {t} tokens"));
    }
    s
}

fn journal(w: &World, id: &str, at: &str, kind: &str, data: Value, made: &str) {
    let db = w.db.lock().unwrap();
    ferb_supervisor::flow::store::journal_row(db.conn(), id, at, kind, &data, Some(made)).unwrap();
}

/// 20 invented exchanges of everyday length in the scenario's chat, the day before.
fn twenty_exchanges(w: &World) {
    for i in 0..20 {
        let (h, m) = (8 + i / 6, (i % 6) * 9);
        journal(
            w,
            &format!("seedmsg{i:04}"),
            &format!("2025-12-31T{h:02}:{m:02}:00+00:00"),
            "chat.message",
            json!({"session_id": "s1", "request_id": format!("seedreq{i:04}"),
                   "text": format!("INVENTED message {i}: can you check whether the invented shop has the blue \
                                    notebook in stock and what it costs, and remind me later today")}),
            "owner",
        );
        journal(
            w,
            &format!("seedrep{i:04}"),
            &format!("2025-12-31T{h:02}:{:02}:30+00:00", m + 1),
            "chat.reply",
            json!({"session_id": "s1", "request_id": format!("seedreq{i:04}"),
                   "text": format!("INVENTED reply {i}: the invented shop lists the blue notebook at 4.50 and has \
                                    three left; I'll remind you at six this evening.")}),
            "ferb,inferred",
        );
    }
}

/// The library block at its size on an invented five-year store in November (ferb-learning's own five-year test
/// holds the shape): its token count.
fn library_in_november() -> usize {
    let tmp = tempfile::tempdir().unwrap();
    let db = ferb_store::Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate_dir(&common::repo().join("engine/runtime/store/migrations"))
        .unwrap();
    let today = "2026-11-27";
    let add = |id: &str, at: &str, kind: &str, data: Value| {
        ferb_supervisor::flow::store::journal_row(db.conn(), id, at, kind, &data, Some("inferred"))
            .unwrap();
    };
    // everyday words, so the token estimate is a real line's (an invented token like "topic3w7" costs several)
    const WORDS: [&str; 40] = [
        "booked", "the", "dentist", "for", "Friday", "morning", "and", "asked", "about", "train",
        "tickets", "to", "Leeds", "checked", "invoice", "from", "landlord", "planned", "weekend",
        "trip", "with", "Robin", "sent", "notes", "meeting", "reminder", "pay", "phone", "bill",
        "looked", "up", "recipe", "dinner", "photos", "backup", "calendar", "moved", "call",
        "next", "week",
    ];
    let line = |n: usize| {
        (0..15)
            .map(|i| WORDS[(n * 7 + i * 3) % WORDS.len()])
            .collect::<Vec<_>>()
            .join(" ")
    };
    for d in 1..=14 {
        let day = ferb_learning::library::shift_day(today, -d);
        let lines: Vec<Value> = (0..6)
            .map(|i| json!({"text": line(d as usize * 10 + i), "refs": []}))
            .collect();
        add(
            &format!("day{d}"),
            &format!("{day}T01:00:00+00:00"),
            "library.day",
            json!({"day": day, "lines": lines}),
        );
    }
    let forty = |p: &str| {
        let mut w: Vec<&str> = vec![p];
        w.extend((0..39).map(|i| WORDS[(i * 5) % WORDS.len()]));
        w.join(" ")
    };
    for m in 1..=10 {
        let month = format!("2026-{m:02}");
        add(
            &format!("month{m}"),
            &format!("{month}-28T01:00:00+00:00"),
            "library.month",
            json!({"month": month, "text": forty("month"), "refs": []}),
        );
    }
    for y in 2021..=2025 {
        add(
            &format!("year{y}"),
            &format!("{y}-12-31T01:00:00+00:00"),
            "library.year",
            json!({"year": y.to_string(), "text": forty("year"), "refs": []}),
        );
    }
    for i in 0..3000 {
        let kind = ["fact", "preference", "person", "project", "env"][i % 5];
        let subject = match kind {
            "person" => format!("person:{}-{i}", WORDS[i % WORDS.len()]),
            "project" => format!("project:{}-{i}", WORDS[(i + 3) % WORDS.len()]),
            _ => "owner".to_owned(),
        };
        db.conn()
            .execute(
                "INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, observed_at, \
                 valid_from, created_at) VALUES (?, ?, ?, ?, 'x', 'owner_stated', 'active', 1.0, ?, ?, ?)",
                rusqlite::params![format!("item{i}"), kind, subject, format!("{}_{}", WORDS[(i + 7) % WORDS.len()], WORDS[(i + 11) % WORDS.len()]),
                                  "2025-01-01T00:00:00+00:00", "2025-01-01T00:00:00+00:00", "2025-01-01T00:00:00+00:00"],
            )
            .unwrap();
    }
    let day_of = |at: &str| at.get(..10).map(str::to_owned);
    let (text, _) = ferb_learning::library::index_text(db.conn(), today, &day_of).unwrap();
    eprintln!(
        "the library in November on a five-year store: {} chars, {} tokens",
        text.chars().count(),
        count_tokens(&text)
    );
    count_tokens(&text)
}

/// The recorded profile (Python-era, in the recordings' instructions) is measured apart and the card's limit stands
/// in for it.
fn without_profile(system: &str) -> (String, usize) {
    match system.find("\n\nTier 1 —") {
        Some(at) => (system[..at].to_string(), count_tokens(&system[at..])),
        None => (system.to_string(), 0),
    }
}

const SCENARIOS: [&str; 6] = [
    "plain_reply",
    "one_step_task",
    "clarify_then_answer",
    "timer_set_and_fired",
    "schedule_created_and_fired",
    "stop_by_chat",
];

// decision: ORCH-726
#[test]
fn what_the_front_door_and_the_planner_are_sent() {
    let mut out = Vec::new();
    for name in SCENARIOS {
        let w = front(name, |_| {});
        for (i, (is_front, system, tools, parts)) in calls(&w).into_iter().enumerate() {
            let who = if is_front { "front door" } else { "planner" };
            out.push(table(
                &format!("{name} call {} ({who})", i + 1),
                &blocks(&system, &tools, &parts),
            ));
        }
    }
    let w = front("plain_reply", twenty_exchanges);
    for (is_front, system, tools, parts) in calls(&w).into_iter().filter(|c| c.0) {
        let _ = is_front;
        out.push(table(
            "plain_reply after 20 exchanges (front door)",
            &blocks(&system, &tools, &parts),
        ));
    }
    eprintln!("{}", out.join("\n"));
}

/// The front door's fixed part and an ordinary turn, in tokens: (fixed, fixed without the tool descriptions, whole).
fn measured() -> (usize, usize, usize) {
    let library = library_in_november();
    let w = front("plain_reply", twenty_exchanges);
    let all = calls(&w);
    let (_, system, tools, parts) = all.iter().find(|c| c.0).expect("a front door call");
    let (rules, recorded_profile) = without_profile(system);
    let tools_tokens = count_tokens(&tool_rows(tools));
    let layer1: usize = parts
        .iter()
        .filter(|p| block_name(p) == "layer 1")
        .map(|p| count_tokens(p))
        .sum();
    let own = count_tokens(&rules) + CARD_TOKENS + layer1 + library;
    let fixed = own + tools_tokens;
    let whole: usize = count_tokens(&rules)
        + CARD_TOKENS
        + tools_tokens
        + parts
            .iter()
            .filter(|p| block_name(p) != "library")
            .map(|p| count_tokens(p))
            .sum::<usize>()
        + library;
    eprintln!(
        "fixed: instructions {} + card (limit) {CARD_TOKENS} + tools {tools_tokens} ({} tools) + layer 1 {layer1} + \
         library {library} = {fixed} tokens (the recording's own profile: {recorded_profile}); an ordinary turn: \
         {whole}",
        count_tokens(&rules),
        tools.len()
    );
    (fixed, own, whole)
}

/// What the front door is sent as measured on 2026-11-27, plus about 5%: guards on the numbers reached after the
/// quality-safe cuts of step 8, not targets to force (the owner, 2026-11-27: "the goal is not to reach too low prompt
/// size just to get faster but get bad results ... just dont loose quality"). The plan's 8k fixed part is not reached:
/// the tool descriptions alone are 6,445 tokens (the six front-door tools take the planner form's parts as schemas,
/// step 5); the rest of the fixed part is 6,116 and an ordinary turn 14,956.
const FIXED_GUARD: usize = 13_200;
const OWN_GUARD: usize = 6_450;
const TURN_GUARD: usize = 15_700;

// decision: ORCH-726
#[test]
fn the_front_doors_fixed_part_and_an_ordinary_turn_stay_under_their_guards() {
    let (fixed, own, whole) = measured();
    assert!(
        whole <= TURN_GUARD,
        "an ordinary turn: {whole} tokens (guard {TURN_GUARD})"
    );
    assert!(
        fixed <= FIXED_GUARD,
        "the front door's fixed part: {fixed} tokens (guard {FIXED_GUARD})"
    );
    assert!(
        own <= OWN_GUARD,
        "the fixed part without its tool descriptions: {own} tokens (guard {OWN_GUARD})"
    );
}
