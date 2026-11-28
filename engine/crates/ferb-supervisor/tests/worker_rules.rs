//! ORCH-719 item 7 and ORCH-758: a worker and a checker get the owner's standing rules and what their task needs, not
//! the whole card the planner reads; nothing newly always on is added to their context (the rules hold every line).
//! Scripted on the recorded one_step_task scenario (planner, worker, checker; fake models, temp store, injected ids
//! and time).

mod common;
mod support_control;

use support_control::{run_with, world_with};

const CARD: &str = "Tier 1 — the card: INVENTED-CARD-FACT the owner's legal name";
const RULES: &str = "Tier 1 — the owner's standing rules: INVENTED-RULE never use the work profile";
const NEWLY_ON: &str = "INVENTED-NEWLY-ALWAYS-ON";

// decision: ORCH-758
#[test]
fn the_planner_reads_the_card_and_a_worker_and_its_checker_the_rules() {
    let mut w = world_with("one_step_task", |_| {});
    *w.daemon.profile.lock().unwrap() = CARD.into();
    *w.daemon.rules.lock().unwrap() = Some(RULES.into());
    *w.daemon.newly_on.lock().unwrap() = NEWLY_ON.into();
    let _ = run_with(&mut w, "one_step_task", |_| {});
    let calls = w.daemon.calls.lock().unwrap().started.clone();
    let (mut planner, mut worker, mut checker) = (0, 0, 0);
    for c in &calls {
        let task = c["task_id"].as_str().unwrap_or("");
        let system = c["system"].as_str().unwrap_or("");
        let parts: String = c["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if task.ends_with(".worker") || task.ends_with(".verify") {
            assert!(system.contains(RULES), "{task}: {system}");
            assert!(!system.contains("INVENTED-CARD-FACT"), "{task}");
            assert!(!parts.contains(NEWLY_ON), "{task}: {parts}");
            if task.ends_with(".worker") {
                worker += 1;
            } else {
                checker += 1;
            }
        } else if system.contains("INVENTED-CARD-FACT") {
            assert!(!system.contains("INVENTED-RULE"), "{task}");
            planner += 1;
        }
    }
    assert!(
        planner >= 1 && worker >= 1 && checker >= 1,
        "planner {planner}, worker {worker}, checker {checker}: {:?}",
        calls
            .iter()
            .map(|c| c["task_id"].clone())
            .collect::<Vec<_>>()
    );
}

// decision: ORCH-758
#[test]
fn without_rules_every_role_reads_the_profile_as_before() {
    let mut w = world_with("one_step_task", |_| {});
    *w.daemon.profile.lock().unwrap() = CARD.into();
    let _ = run_with(&mut w, "one_step_task", |_| {});
    let calls = w.daemon.calls.lock().unwrap().started.clone();
    let workers: Vec<_> = calls
        .iter()
        .filter(|c| c["task_id"].as_str().unwrap_or("").ends_with(".worker"))
        .collect();
    assert!(!workers.is_empty());
    for c in workers {
        assert!(c["system"].as_str().unwrap().contains("INVENTED-CARD-FACT"));
    }
}
