//! The worker's and checker's context, run by run, against the 80 worker, checker, fix and re-check runs of the 28
//! daemon scenarios (rust/fixtures/supervisor/scenarios.json): `_context` from the reads make.py recorded at each
//! call, the checker's report block and code facts, the runtime-tools block every run ends with, and every model
//! call's context assembled in its window (the parts the model received).
//!
//! Inputs not built here: the memory blocks (`_worker_memory`, none in these scenarios), the earlier results
//! (`_result_blocks`, none), the recorded tool calls (evidence.py `recorded_calls`, ferb-orchestrator's) and the
//! changed-file blocks (`_changed_files`, which need the change store): their recorded outputs stand in.

mod common;

use common::*;
use ferb_clock::zone::Zone;
use ferb_context::assemble::ContextBlock;
use ferb_context::tokens::Tokenizer;
use ferb_supervisor::planner::assemble_run;
use ferb_supervisor::prompts::runtime_tools_block;
use ferb_supervisor::worker_context::*;
use serde_json::{Map, Value};

struct Recorded<'a> {
    calls: Vec<&'a Value>,
    at: usize,
    what: String,
}

impl SourceBlocks for Recorded<'_> {
    fn store(&mut self, call: &SourceCall) -> String {
        let rec = self
            .calls
            .get(self.at)
            .unwrap_or_else(|| panic!("{}: an unrecorded source block", self.what));
        self.at += 1;
        assert_eq!(call.label, s(&rec["label"]), "{}", self.what);
        assert_eq!(call.text, s(&rec["text"]), "{}: {}", self.what, call.label);
        assert_eq!(
            call.origin,
            s(&rec["origin"]),
            "{}: {}",
            self.what,
            call.label
        );
        assert_eq!(
            call.source_refs,
            strs(&rec["source_refs"]),
            "{}: {}",
            self.what,
            call.label
        );
        assert_eq!(
            call.payload,
            rec["payload"].as_str().map(str::to_string),
            "{}: {}",
            self.what,
            call.label
        );
        s(&rec["reference"])
    }
}

fn obj(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn blocks(v: &Value) -> Vec<ContextBlock> {
    v.as_array()
        .map(|a| a.iter().map(block).collect())
        .unwrap_or_default()
}

fn reads(r: &Value) -> ContextReads {
    let calls: Vec<&Value> = r["calls"].as_array().unwrap().iter().collect();
    let out_of = |kind: &str| {
        calls
            .iter()
            .find(|c| c["kind"] == kind)
            .map(|c| c["out"].clone())
    };
    let t = &r["targets"];
    ContextReads {
        tree_id: s(&r["tree_id"]),
        worker_id: s(&r["worker_id"]),
        learned: r["learned"].as_bool().unwrap(),
        words: s(&r["words"]),
        payload: s(&r["payload"]),
        activity: Some(Activity::from_json(&r["activity"])),
        schedule_id: r["schedule_id"].as_str().map(str::to_string),
        schedule_messages: out_of("_schedule_messages")
            .map(|v| s(&v))
            .unwrap_or_default(),
        parent_goal: s(&r["parent_goal"]),
        goal: s(&r["goal"]),
        worker_type: s(&r["worker_type"]),
        step_context: s(&r["step_context"]),
        intake_receipts: r["intake_receipts"].as_array().cloned().unwrap_or_default(),
        disturbance: s(&r["disturbance"]),
        schedule_facts: obj(&r["schedule_facts"]),
        reports: r["reports"].as_array().cloned().unwrap_or_default(),
        task_made: calls
            .iter()
            .find(|c| c["kind"] == "source_block" && c["label"] == "Previous attempts (data)")
            .map(|c| s(&c["origin"]))
            .unwrap_or_default(),
        unresolved: r["unresolved"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|rows| rows.as_array().unwrap().iter().map(obj))
            .collect(),
        wait_cut: r["wait_cut"].as_str().map(str::to_string),
        conditions: strs(&r["conditions"]),
        targets: [
            strs(&t["apps"]),
            strs(&t["sites"]),
            strs(&t["paths"]),
            strs(&t["recipients"]),
        ],
        folders: strs(&r["folders"]),
        workspace: s(&r["workspace"]),
        posted: r["posted"].as_array().cloned().unwrap_or_default(),
        plan_steps: if r["plan"].as_bool().unwrap() {
            Some(r["plan_steps"].clone())
        } else {
            None
        },
        saved_plan: r["saved_plan"].as_object().cloned(),
        owner_questions: r["owner_questions"].as_array().cloned().unwrap_or_default(),
        reopened: set(&r["reopened"]),
        confirmations: r["confirmations"]
            .as_array()
            .unwrap()
            .iter()
            .map(obj)
            .collect(),
        memory: out_of("_worker_memory")
            .map(|v| blocks(&v))
            .unwrap_or_default(),
        results: out_of("_result_blocks")
            .map(|v| blocks(&v))
            .unwrap_or_default(),
    }
}

fn verifier(r: &Value) -> VerifierReads {
    VerifierReads {
        owner_answer: s(&r["owner_answer"]),
        judged: r["judged"]
            .as_array()
            .unwrap()
            .iter()
            .map(|j| (j[0].as_u64().unwrap() as usize, s(&j[1]), s(&j[2])))
            .collect(),
        confirmed: obj(&r["confirmed"]),
        done: r["outcome_status"] == "done",
        artifacts: strs(&r["artifacts"]),
        reason: s(&r["reason"]),
        worker_question: s(&r["worker_question"]),
        part_updates: r["part_updates"].as_array().cloned().unwrap_or_default(),
        worker_types: strs(&r["worker_types"]),
        availability: s(&r["availability"]),
        observations: r["observations"].as_array().cloned().unwrap_or_default(),
        calls_since: r["calls_since"].as_i64().unwrap_or(0),
        began: r["began"].as_array().map(|b| s(&b[0])),
    }
}

fn time(v: &Value) -> Option<ferb_clock::py::Time> {
    v.as_str().and_then(ferb_clock::py::parse)
}

#[test]
fn every_worker_and_checker_run() {
    let zone = Zone::utc();
    let root = root();
    let daemon = read("rust/fixtures/daemon/chat_scenarios.json");
    let (mut contexts, mut reports, mut facts, mut tools, mut assembled) = (0, 0, 0, 0, 0);
    for (name, records) in scenarios() {
        let calls = daemon["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sc| sc["name"] == name.as_str())
            .unwrap();
        let model_calls = calls["model_calls"].as_array().unwrap();
        let mut n_assemble = 0;
        let mut run: Option<&Value> = None;
        let mut report: Option<String> = None;
        for r in records {
            match r["kind"].as_str().unwrap() {
                "context" => {
                    let what = format!("{name} {}", s(&r["tree_id"]));
                    let source_calls: Vec<&Value> = r["calls"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|c| c["kind"] == "source_block")
                        .collect();
                    let mut sources = Recorded {
                        calls: source_calls,
                        at: 0,
                        what: what.clone(),
                    };
                    let got = worker_context(&reads(r), &mut sources, &wrap);
                    assert_eq!(
                        sources.at,
                        sources.calls.len(),
                        "{what}: every source block made"
                    );
                    let want = blocks(&r["out"]);
                    assert_eq!(got.len(), want.len(), "{what}: how many blocks");
                    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                        assert_eq!(g, w, "{what}: block {i}");
                    }
                    contexts += 1;
                }
                "verifier" => {
                    report = Some(verifier_report(&verifier(r), &wrap));
                }
                "code_facts" => {
                    let got = code_facts(&obj(&r["facts"]), r["tabs"].as_array().unwrap(), &wrap);
                    assert_eq!(vec![got], blocks(&r["out"]), "{name}: code facts");
                    facts += 1;
                }
                "run_worker" => run = Some(r),
                "assemble" => {
                    let call = &model_calls[n_assemble];
                    n_assemble += 1;
                    let what = format!("{name} call {} ({})", n_assemble, s(&call["key"]));
                    let ctx = blocks(&r["blocks"]);
                    if let Some(w) = run.take() {
                        // the runtime-tools block (worker.py:422-424): the offered tools and the time facts
                        let now = time(&w["now"]).unwrap();
                        let (active, last) = (time(&w["owner_active"]), time(&w["last_event"]));
                        let deadline = time(&w["deadline"]);
                        let head = ferb_clock::context::time_block(
                            &now,
                            &zone.abbreviation(&now),
                            &ferb_clock::context::Facts {
                                task_started: Some(&now),
                                deadline: deadline.as_ref(),
                                owner_last_active: active.as_ref(),
                                last_trail_event: last.as_ref(),
                                ages: &[],
                                since_last_talk: None,
                            },
                        );
                        let names = strs(&resolve(&call["tools"]));
                        let want = runtime_tools_block(&root, &names, &head).unwrap();
                        let block = ctx
                            .iter()
                            .find(|b| b.reference == "runtime tools and time facts")
                            .unwrap_or_else(|| panic!("{what}: no runtime tools block"));
                        assert_eq!(block.text, want, "{what}: runtime tools");
                        tools += 1;
                        if w["role"] == "verifier" && w["prompt"] == "verifier.md" {
                            let made = report
                                .clone()
                                .unwrap_or_else(|| panic!("{what}: no verifier record"));
                            let block = ctx
                                .iter()
                                .find(|b| b.reference == "complete report and inspection evidence")
                                .unwrap_or_else(|| panic!("{what}: no report block"));
                            assert_eq!(block.text, made, "{what}: the checker's report");
                            reports += 1;
                        }
                    }
                    let assembly = assemble_run(
                        ctx,
                        &[],
                        r["budget_tokens"].as_i64().unwrap(),
                        r["reserved_tokens"].as_i64().unwrap(),
                        r["max_chars"].as_i64(),
                        Tokenizer::Utf8Bytes,
                    )
                    .unwrap();
                    assert_eq!(assembly.parts, strs(&r["parts"]), "{what}: parts");
                    assembled += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        (contexts, reports, facts, tools, assembled),
        (78, 36, 35, 80, 118)
    );
}

/// The fix turn's and the re-check's blocks (supervisor.py:6582, :6616) in `checker_findings_fix`, built from the
/// scripted answers the model gave: the worker's answer, the checker's findings, the fix turn's revision.
#[test]
fn fix_and_recheck_blocks() {
    let daemon = read("rust/fixtures/daemon/chat_scenarios.json");
    let sc = daemon["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "checker_findings_fix")
        .unwrap();
    let calls = sc["model_calls"].as_array().unwrap();
    let final_of = |key: &str| -> Value {
        let call = calls.iter().find(|c| c["key"] == key).unwrap();
        let text = call["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["kind"] == "final")
            .unwrap()["data"]["text"]
            .clone();
        serde_json::from_str(&s(&resolve(&text))).unwrap()
    };
    let answer = s(&final_of("worker")["answer"]);
    let findings = final_of("worker.verify")["findings"]
        .as_array()
        .unwrap()
        .clone();
    let revised = s(&final_of("worker.fix")["answer"]);
    let records = &scenarios()
        .iter()
        .find(|(n, _)| n == "checker_findings_fix")
        .unwrap()
        .1;
    let assembled: Vec<&Value> = records.iter().filter(|r| r["kind"] == "assemble").collect();
    let keys: Vec<String> = calls.iter().map(|c| s(&c["key"])).collect();
    let ctx_of =
        |key: &str| blocks(&assembled[keys.iter().position(|k| k == key).unwrap()]["blocks"]);
    let fix = fix_block(&answer, &findings, &wrap);
    assert!(ctx_of("worker.fix").contains(&fix), "the fix turn's block");
    let recheck = recheck_block(&findings, &revised, &wrap);
    assert_eq!(ctx_of("worker.recheck")[0], recheck, "the re-check's block");
}
