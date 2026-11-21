//! The owner asked from his phone, 2026-11-22: "so yesterday my amc was on power and the lid was closed but keep mac on
//! wasnt on, BUT still i could chat to you, how>>>>???". FERB guessed that it held the Mac awake itself (it doesn't:
//! outside Keep my Mac working it holds nothing with the lid closed) and said it had no record of sleep or wake. The
//! cause was macOS's Power Nap: on power the sleeping Mac woke briefly every few minutes and FERB answered then.
//!
//! A recorded night in the store, as the insomnia area's power watch journals it (`mac.power`, `insomnia.state`), and
//! device.facts (the planner's and workers' read tool) called through the real `ferbd daemon` program for that
//! night: the facts it gives the model name Power Nap, the brief wakes and the mode being off. What the model then
//! says is not tested (TEST-500). Temporary folders only; the stand-ins' clock and no Mac parts.

mod shim;
mod support;

use serde_json::{json, Value};
use shim::{all_scopes, Shim};
use support::*;

/// A night as the power watch records it: the mode turned off in the evening, the lid closed on power with Power Nap
/// on, 212 brief wakes, the lid opened in the morning.
fn night() -> Vec<(String, Vec<Value>)> {
    let row = |id: &str, at: &str, kind: &str, data: Value| {
        (
            "INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)".to_string(),
            vec![json!(id), json!(at), json!(kind), json!(data.to_string())],
        )
    };
    vec![
        row(
            "n1",
            "2026-11-21T20:10:00+05:30",
            "insomnia.state",
            json!({"state": "ended", "why": "turned_off", "by": "mac"}),
        ),
        row(
            "n2",
            "2026-11-21T22:23:58+05:30",
            "mac.power",
            json!({"event": "lid", "closed": true}),
        ),
        row(
            "n3",
            "2026-11-21T22:24:00+05:30",
            "mac.power",
            json!({"event": "asleep", "lid_closed": true, "on_battery": false, "power_nap": true}),
        ),
        row(
            "n4",
            "2026-11-22T07:10:00+05:30",
            "mac.power",
            json!({"event": "awake", "asleep_since": "2026-11-21T22:24:00+05:30", "brief_wakes": 212}),
        ),
        row(
            "n5",
            "2026-11-22T07:10:01+05:30",
            "mac.power",
            json!({"event": "lid", "closed": false}),
        ),
    ]
}

// decisions: SENSE-426, SENSE-427, SENSE-428, SENSE-429
#[test]
fn the_owners_lid_closed_question_gets_facts_that_name_power_nap() {
    let tmp = root();
    let run = Run::start(tmp.path(), "01", json!({"ids": "id"}));
    run.control(json!({"do": "sql", "statements": night()}));
    let token = run.control(
        json!({"do": "issue", "task_id": "task-1", "worker_id": "w-1",
                                   "identity": "owner", "scopes": all_scopes()}),
    );
    run.control(json!({"do": "hold_task", "task_id": "task-1"}));
    let mut owner = Shim::open(&run, token.as_str().unwrap());
    let got = owner.call(
        "device.facts",
        json!({"since": "2026-11-21", "until": "2026-11-22",
               "why": "the owner asks how he could chat with FERB yesterday with the lid closed and the mode off"}),
    );
    assert_eq!(got["is_error"], false, "{got}");
    let facts = &got["content"];
    let record = &facts["sleep_record"];
    let entries = record["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{facts}"));
    let events: Vec<&str> = entries.iter().filter_map(|e| e["event"].as_str()).collect();
    assert_eq!(
        events,
        [
            "Keep my Mac working turned off",
            "lid closed",
            "went to sleep",
            "woke fully",
            "lid opened"
        ],
        "{record}"
    );
    let asleep = &entries[2];
    assert_eq!(asleep["lid"], "closed");
    assert_eq!(asleep["power_source"], "power adapter");
    assert_eq!(asleep["power_nap"], "on");
    assert_eq!(entries[3]["brief_wakes"], 212);
    assert!(record["about"].as_str().unwrap().contains("Power Nap"));
    // FERB's own mode: off now, and what FERB does outside it (it never keeps a closed Mac awake)
    let mode = &facts["keep_my_mac_working"];
    assert_eq!(mode["on"], false);
    assert!(mode["when_off"]
        .as_str()
        .unwrap()
        .contains("never keeps it awake once the lid is closed"));
    // a malformed stretch is the caller's error before anything is read
    let bad = owner.call("device.facts", json!({"since": "last night"}));
    assert_eq!(bad["is_error"], true, "{bad}");
}
