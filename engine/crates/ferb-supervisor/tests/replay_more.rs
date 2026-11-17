//! The supervisor's paths no earlier recording reached, replayed against the recordings
//! engine/fixtures/supervisor/make_more.py made from the real Python daemon (more_scenarios.json): the running
//! summary's fold, several asks and their card, the checklist, memory and forget side entries, an answer routed in
//! chat, loops, the paused task's mention, a closed tab, findings on a reason, a runtime switch, an event wait, and
//! the rest named in that script. The harness is tests/replay.rs's (tests/replay/harness.rs): every frame,
//! every model call, every tool call's reply, the chat RPCs' replies and every table but those another part owns.

mod common;
#[path = "replay/harness.rs"]
mod harness;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use common::*;
use harness::*;
use serde_json::{json, Map, Value};

fn more() -> &'static Value {
    static M: OnceLock<Value> = OnceLock::new();
    M.get_or_init(|| {
        let text =
            std::fs::read_to_string(repo().join("engine/fixtures/supervisor/more_scenarios.json"))
                .unwrap();
        // ORCH-642 (a named difference): make_more.py's event wait waits on an invented topic nothing publishes,
        // which the port refuses at admission; the scenario replays with a topic FERB does publish, everywhere it
        // appears (the plan, the step's state, the stored plan and event, the event the test delivers)
        // and APP-926's (named_diffs::app_926): owner-facing words that spoke of AI
        named_diffs::app_926(
            serde_json::from_str(&text.replace("browser.download_finished", EVENT_WAIT_TOPIC))
                .expect("fixture JSON"),
        )
    })
}

/// The topic event_wait replays with (ORCH-642).
const EVENT_WAIT_TOPIC: &str = "timer.fired";

/// A scenario's kept records (make_more.py `kept_records`), each role's instruction inputs put back.
fn records(name: &str) -> Vec<Value> {
    let inputs = &more()["inputs"];
    more()["records"][name]
        .as_array()
        .unwrap_or_else(|| panic!("no records for {name}"))
        .iter()
        .map(|r| {
            let mut r = resolve(r);
            if let Some(key) = r["inputs"]["inputs"].as_str() {
                r["inputs"] = resolve(&inputs[key]);
            }
            r
        })
        .collect()
}

/// The daemon's registered tools and the answer schemas, as tests/replay.rs builds them.
fn tools_and_schemas() -> (Vec<Tool>, Map<String, Value>) {
    let chat = read("engine/fixtures/daemon/chat_scenarios.json");
    let catalog = read("engine/fixtures/catalog/tools.json");
    let registered: BTreeSet<String> = chat["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .chain(more()["scenarios"].as_array().unwrap().iter())
        .flat_map(|s| s["model_calls"].as_array().unwrap().iter())
        .filter(|m| m["role_played"] != "planner")
        .flat_map(|m| strs(&resolve(&m["tools"])))
        .chain(
            [
                "about.read",
                "chat.history",
                "memory.read",
                "task.evidence",
                "trail.days",
                "trail.query",
            ]
            .map(String::from),
        )
        .collect();
    let tools = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| registered.contains(t["name"].as_str().unwrap()))
        .map(|t| Tool {
            name: s(&t["name"]),
            row: json!({"name": t["name"], "description": t["description"], "input_schema": t["input_schema"]}),
            scopes: strs(&t["scopes"]),
            identities: strs(&t["identities"]),
            observation: t["facts"]["observation"].as_bool().unwrap_or(false),
        })
        .collect();
    let answers = read("engine/fixtures/catalog/answer_schemas.json");
    let schemas = answers["schemas"]
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(k, v)| v.get("core").map(|c| (k.clone(), c.clone())))
        .collect();
    (tools, schemas)
}

/// The scenario's folder number as the recorder numbered it (`/FX/NN/home`; a scenario that names no path: any).
fn folder_index(sc: &Value) -> usize {
    let text = sc.to_string();
    text.find("/FX/")
        .and_then(|at| text[at + 4..at + 6].parse::<usize>().ok())
        .map_or(0, |n| n - 1)
}

fn replay(name: &str) -> Report {
    let chat = read("engine/fixtures/daemon/chat_scenarios.json");
    let (_folder, run) = RunRoot::new(recorded_root_len(&chat));
    let run = Arc::new(run);
    let (tools, schemas) = tools_and_schemas();
    let sc = more()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    let r = run_scenario(sc, &records(name), folder_index(sc), &run, &tools, &schemas);
    eprintln!("{name}: {} comparisons", r.checked);
    if let Some(stop) = &r.stopped {
        eprintln!("  stops at {stop}");
    }
    for f in r.frames.iter().take(3) {
        eprintln!("  frames: {f}");
    }
    for f in r.calls.iter().take(3) {
        eprintln!("  model calls: {f}");
    }
    for f in r.tables.iter().take(6) {
        eprintln!("  tables: {f}");
    }
    if !r.skipped.is_empty() {
        eprintln!("  not compared: {}", r.skipped.join(", "));
    }
    r
}

fn full(name: &str) {
    let r = replay(name);
    assert!(r.full(), "{name} does not replay fully");
}

// decision: ORCH-303
// decision: ORCH-326
#[test]
fn summary_fold() {
    full("summary_fold");
}

// decision: ORCH-319
// decision: ORCH-543
#[test]
fn several_asks() {
    full("several_asks");
}

// decision: ORCH-532
#[test]
fn plan_parts_checklist() {
    full("plan_parts_checklist");
}

#[test]
fn remember_then_forget() {
    full("remember_then_forget");
}

// decision: ORCH-537
#[test]
fn answer_in_chat() {
    full("answer_in_chat");
}

// decision: ORCH-626
#[test]
fn checklist_single() {
    full("checklist_single");
}

// decision: ORCH-322
#[test]
fn ask_step() {
    full("ask_step");
}

// decision: ORCH-322
#[test]
fn wait_step() {
    full("wait_step");
}

#[test]
fn route_question() {
    full("route_question");
}

// decision: ORCH-315
#[test]
fn checker_runtime_down() {
    full("checker_runtime_down");
}

// decision: ORCH-558
#[test]
fn last_call_empty() {
    full("last_call_empty");
}

// decision: ORCH-553
#[test]
fn unbacked_part() {
    full("unbacked_part");
}

// decision: ORCH-538
#[test]
fn correct_pending_check() {
    full("correct_pending_check");
}

// decision: ORCH-619
#[test]
fn earlier_result() {
    full("earlier_result");
}

// decision: ORCH-505
// decision: ORCH-322
#[test]
fn loop_steer_finish() {
    full("loop_steer_finish");
}

// decision: ORCH-323
// decision: ORCH-536
#[test]
fn loop_pause_stop() {
    full("loop_pause_stop");
}

// decision: ORCH-312
#[test]
fn mention_paused() {
    full("mention_paused");
}

// decision: ORCH-536
#[test]
fn owner_closed_tab() {
    full("owner_closed_tab");
}

// decision: ORCH-562
#[test]
fn reason_findings() {
    full("reason_findings");
}

// decision: ORCH-315
#[test]
fn runtime_switch_tap() {
    full("runtime_switch_tap");
}

// decision: ORCH-322
#[test]
fn event_wait() {
    full("event_wait");
}

// decision: ORCH-301
// decision: ORCH-322
#[test]
fn conversation_handover() {
    full("conversation_handover");
}

// decision: ORCH-301
// decision: ORCH-520
// decision: ORCH-649
#[test]
fn conversation_change() {
    full("conversation_change");
}

// decision: ORCH-503
// decision: ORCH-306
#[test]
fn stated_scope() {
    full("stated_scope");
}

// decision: ORCH-524
// decision: DMN-520
// decision: ORCH-529
// decision: GATE-504
#[test]
fn front_window() {
    full("front_window");
}

// decision: ORCH-500
// decision: SENSE-2
#[test]
fn preview_attention() {
    full("preview_attention");
}

// decision: APP-777
#[test]
fn browse_as_corrected() {
    full("browse_as_corrected");
}

// decision: ORCH-505
// decision: ORCH-506
#[test]
fn probe_watch() {
    full("probe_watch");
}

#[test]
fn login_sites() {
    full("login_sites");
}

#[test]
fn displaced_question() {
    full("displaced_question");
}

// decision: ORCH-580
// decision: ORCH-581
#[test]
fn inventory_walk() {
    full("inventory_walk");
}

// decision: ORCH-301
#[test]
fn conversation_resume() {
    full("conversation_resume");
}

// decision: DMN-520
#[test]
fn capture_timeout() {
    full("capture_timeout");
}

// ================= GATE-609: the planner reads a safety check's box for the sites it names (GATE-606 c) =================

/// `correct_pending_check` replayed with the planner reading its card reply (an invented change of the recording: its
/// comparison is not asserted), the card asking about github.com. The owner types `words` in the pending
/// confirmation's box; the planner's scripted
/// answer is the recording's plan made a correction of the running task (`continue`) whose sites are `sites`, or
/// `control` replaced as given. Returns the run's report and the task's id.
fn card_reply(words: &str, sites: &[&str], control: Option<Value>) -> (Report, String) {
    let chat = read("engine/fixtures/daemon/chat_scenarios.json");
    let (_folder, run) = RunRoot::new(recorded_root_len(&chat));
    let run = Arc::new(run);
    let (tools, schemas) = tools_and_schemas();
    let mut sc = more()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "correct_pending_check")
        .unwrap()
        .clone();
    sc["reads_card_replies"] = json!(true);
    // the card asks about github.com, as the live one did (its needs' host, `Approval::site_hosts`)
    sc["card_site_hosts"] = json!(["github.com"]);
    let mut task_id = String::new();
    for st in sc["steps"].as_array_mut().unwrap() {
        let Some(lines) = st["lines"].as_array_mut() else {
            continue;
        };
        if !lines
            .iter()
            .any(|l| s(&l["send"]).contains("\"chat.correct\""))
        {
            continue;
        }
        let mut sent: Value = serde_json::from_str(&s(&lines[0]["send"])).unwrap();
        sent["params"]["text"] = json!(words);
        lines[0]["send"] = json!(sent.to_string());
        let got: Value = serde_json::from_str(&s(&lines[1]["recv"])).unwrap();
        task_id = s(&got["result"]["task_id"]);
    }
    assert!(!task_id.is_empty());
    let calls = sc["model_calls"].as_array_mut().unwrap();
    let mut reading = resolve(&calls[0]);
    let events = reading["events"].as_array_mut().unwrap();
    for e in events.iter_mut() {
        if e["kind"] == "final" {
            let mut out: Value = serde_json::from_str(&s(&e["data"]["text"])).unwrap();
            out["control"] = match control.clone() {
                // "TASK" stands for the running task's id
                Some(mut c) if c["task_id"] == "TASK" => {
                    c["task_id"] = json!(task_id);
                    c
                }
                Some(c) => c,
                None => json!({"task_id": task_id, "action": "continue", "new_goal": false,
                               "standing_sends": "unchanged", "supersedes": []}),
            };
            out["task"]["targets"]["sites"] = json!(sites);
            out["text"] = json!("Noted.");
            e["data"]["text"] = json!(ferb_py::json::dumps(&out));
        }
    }
    // the planner's reading is the third call made: after the plan and the worker waiting on the card
    calls.insert(2, reading);
    let mut recs = records("correct_pending_check");
    let system = recs.iter().find(|r| r["kind"] == "system").unwrap().clone();
    let third = recs
        .iter()
        .enumerate()
        .filter(|(_, r)| r["kind"] == "system")
        .nth(2)
        .map(|(i, _)| i)
        .unwrap();
    recs.insert(third, system);
    let supervise = recs
        .iter()
        .find(|r| r["kind"] == "supervise")
        .unwrap()
        .clone();
    recs.push(supervise);
    let r = run_scenario(&sc, &recs, folder_index(&sc), &run, &tools, &schemas);
    (r, task_id)
}

fn rows<'a>(r: &'a Report, kind: &str) -> Vec<&'a Value> {
    r.journal
        .iter()
        .filter(|(k, _)| k == kind)
        .map(|(_, d)| d)
        .collect()
}

/// The reply FERB wrote to the box's words, and what `task.card_sites` recorded.
fn told(r: &Report) -> (String, Value) {
    let corrected = rows(r, "chat.message")
        .into_iter()
        .find(|d| d.get("correction_of").is_some())
        .map(|d| s(&d["request_id"]))
        .unwrap();
    let reply = rows(r, "chat.reply")
        .into_iter()
        .find(|d| s(&d["request_id"]) == corrected)
        .map(|d| s(&d["text"]))
        .unwrap();
    let sites = rows(r, "task.card_sites")
        .first()
        .map(|d| (*d).clone())
        .unwrap_or(Value::Null);
    (reply, sites)
}

/// The card's question was set aside, never answered.
fn set_aside(r: &Report) {
    let resolved = rows(r, "approval.resolved");
    assert!(
        resolved.iter().any(|d| d["state"] == "superseded"),
        "{resolved:?}"
    );
    assert!(
        !resolved.iter().any(|d| d["state"] == "granted"),
        "a box's words are never a Yes: {resolved:?}"
    );
}

/// The task's request as recorded last (its sites).
fn request_sites(r: &Report) -> Vec<String> {
    let mut all: Vec<String> = r.sites.values().flatten().cloned().collect();
    all.sort();
    all
}

/// The live case's words: the planner reads github.com, the card asked about github.com and the owner's words name
/// it ("GitHub"): named, said so, the card set aside (never a Yes), and the planner had code's fact about the question
/// beside the owner's words.
// decision: GATE-609
// decision: GATE-611
// decision: GATE-606
#[test]
fn a_box_reply_naming_the_asked_site_names_it() {
    let (r, _) = card_reply("use my Brave GitHub", &["github.com"], None);
    let (reply, sites) = told(&r);
    assert_eq!(sites["named"], json!(["github.com"]), "{sites}");
    assert_eq!(
        reply,
        "Got it: github.com is a site you named for this task."
    );
    assert_eq!(request_sites(&r), ["github.com"]);
    set_aside(&r);
    let reading = r
        .started
        .iter()
        .find(|c| c["n"] == 2)
        .expect("the planner's reading");
    let parts = strs(&reading["parts"]).join("");
    assert!(parts.contains("use my Brave GitHub"), "{parts}");
    assert!(
        parts.contains(
            "Typed in the box of FERB's open question ap-a00000000001, never a Yes or No to it."
        ),
        "{parts}"
    );
}

const TYPE_IT: &str = "Got it. To name a site for this task, type its address (like example.com).";

/// "yes please", "1", "ok", "that one", "the site above": whatever the planner returns, the owner's words name no
/// site, and FERB says how to name one. (A bare "Yes" is the option itself, GATE-510, and never reaches the planner.)
// decision: GATE-609
// decision: GATE-611
// decision: GATE-510
// decision: GATE-512
#[test]
fn a_box_reply_without_the_sites_words_names_nothing() {
    for words in ["yes please", "1", "ok", "that one", "the site above"] {
        let (r, _) = card_reply(words, &["github.com"], None);
        let (reply, sites) = told(&r);
        assert_eq!(sites["named"], json!([]), "{words}: {sites}");
        assert_eq!(sites["refused"], json!(["github.com"]), "{words}: {sites}");
        assert_eq!(reply, TYPE_IT, "{words}");
        assert!(request_sites(&r).is_empty(), "{words}");
        set_aside(&r);
    }
}

/// A refusal the planner reads without the site, or as no correction, names nothing. A redirect to another site names
/// it only when the owner typed its address: "not github, use gitlab" names nothing (FERB says to type the address);
/// "use gitlab.com instead" names gitlab.com, never github.com.
// decision: GATE-609
// decision: GATE-611
#[test]
fn a_refusal_names_nothing_and_a_redirect_names_only_a_typed_address() {
    let (r, _) = card_reply("don't send it to github", &[], None);
    let (reply, sites) = told(&r);
    assert_eq!(sites["named"], json!([]));
    assert_eq!(reply, "Got it.");
    assert!(request_sites(&r).is_empty());
    set_aside(&r);
    let (r, _) = card_reply(
        "don't send it to github",
        &["github.com"],
        Some(json!(null)),
    );
    assert_eq!(told(&r).1["why"], json!("no correction of a task"));
    assert!(request_sites(&r).is_empty());
    let (r, _) = card_reply("not github, use gitlab", &["gitlab.com"], None);
    let (reply, sites) = told(&r);
    assert_eq!(sites["refused"], json!(["gitlab.com"]), "{sites}");
    assert_eq!(reply, TYPE_IT);
    assert!(request_sites(&r).is_empty());
    let (r, _) = card_reply("not github, use gitlab.com instead", &["gitlab.com"], None);
    let (reply, sites) = told(&r);
    assert_eq!(sites["named"], json!(["gitlab.com"]), "{sites}");
    assert_eq!(
        reply,
        "Got it: gitlab.com is a site you named for this task."
    );
    assert_eq!(request_sites(&r), ["gitlab.com"]);
}

/// The review's case end to end: the owner declines in words, an injected planner names a site the card never asked
/// about whose zone label is an ordinary word ("it" in evil.it.com): refused, nothing named.
// decision: GATE-611
#[test]
fn a_decline_with_an_injected_site_names_nothing() {
    let (r, _) = card_reply("no, skip it and just finish", &["evil.it.com"], None);
    let (reply, sites) = told(&r);
    assert_eq!(sites["named"], json!([]), "{sites}");
    assert_eq!(sites["refused"], json!(["evil.it.com"]), "{sites}");
    assert_eq!(reply, TYPE_IT);
    assert!(request_sites(&r).is_empty());
    set_aside(&r);
}

/// The planner names sites the owner never typed: the card's own host without its name in the words, and another
/// host by a word: both refused.
// decision: GATE-609
// decision: GATE-611
#[test]
fn a_site_the_planner_adds_that_the_owner_never_typed_is_refused() {
    let (r, _) = card_reply("use my Brave account", &["github.com", "brave.com"], None);
    let (reply, sites) = told(&r);
    assert_eq!(sites["named"], json!([]), "{sites}");
    assert_eq!(
        sites["refused"],
        json!(["github.com", "brave.com"]),
        "{sites}"
    );
    assert_eq!(reply, TYPE_IT);
    assert!(request_sites(&r).is_empty());
}

/// A stop, or a correction of another task, names nothing.
// decision: GATE-609
#[test]
fn a_stop_or_another_tasks_correction_names_nothing() {
    let stop = json!({"task_id": "TASK", "action": "stop", "new_goal": false, "standing_sends": "unchanged",
                      "supersedes": []});
    let (r, _) = card_reply("stop, use github another day", &["github.com"], Some(stop));
    assert_eq!(told(&r).1["why"], json!("control stop"));
    assert!(request_sites(&r).is_empty());
    let other = json!({"task_id": "id-999999", "action": "continue", "new_goal": false,
                       "standing_sends": "unchanged", "supersedes": []});
    let (r, _) = card_reply("use github", &["github.com"], Some(other));
    assert_eq!(told(&r).1["why"], json!("another task"));
    assert!(request_sites(&r).is_empty());
}
