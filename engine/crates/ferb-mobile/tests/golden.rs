//! The acceptance test of the phone's fold (S7): every scenario of the three chat-entries fixture files, folded here,
//! equals what FERB.app's fold shows for it (`engine/fixtures/chat-entries/transcripts.json`, written by
//! `EntryFoldTranscriptTests.swift`), compared as JSON values. The Mac's behaviour is the golden.

mod common;

use common::{read, scenarios};
use ferb_mobile::entry::ChatEntry;
use ferb_mobile::fold;
use serde_json::Value;

/// The zone the golden was written in (FERB.app's `Calendar.current` on the machine that ran the Swift test): it
/// decides only whether a later answer was said on its turn's day.
const GOLDEN_UTC_OFFSET_S: i32 = 5 * 3600 + 30 * 60;

fn first_difference(path: &str, golden: &Value, ours: &Value) -> Option<String> {
    match (golden, ours) {
        (Value::Object(g), Value::Object(o)) => {
            for k in g.keys().chain(o.keys()) {
                match (g.get(k), o.get(k)) {
                    (Some(a), Some(b)) => {
                        if let Some(d) = first_difference(&format!("{path}.{k}"), a, b) {
                            return Some(d);
                        }
                    }
                    (a, b) => return Some(format!("{path}.{k}: golden {a:?}, ours {b:?}")),
                }
            }
            None
        }
        (Value::Array(g), Value::Array(o)) if g.len() == o.len() => g
            .iter()
            .zip(o)
            .enumerate()
            .find_map(|(i, (a, b))| first_difference(&format!("{path}[{i}]"), a, b)),
        _ if golden == ours => None,
        _ => Some(format!("{path}: golden {golden}, ours {ours}")),
    }
}

/// Each conversation of the entries, as the golden lists them.
fn ours(entries: &[ChatEntry]) -> Vec<Value> {
    let mut conversations: Vec<&str> = entries.iter().map(|e| e.conversation_id.as_str()).collect();
    conversations.sort();
    conversations.dedup();
    conversations
        .into_iter()
        .map(|c| {
            let mine: Vec<ChatEntry> = entries
                .iter()
                .filter(|e| e.conversation_id == c)
                .cloned()
                .collect();
            let t = fold::transcript(&mine, GOLDEN_UTC_OFFSET_S);
            serde_json::json!({"conversation": c, "items": t.items, "questions": t.questions})
        })
        .collect()
}

// decision: ENG-22
// decision: ENG-9
// decision: ENG-10
// decision: PHN-401
// decision: PHN-473
#[test]
fn every_scenario_folds_to_what_the_mac_shows() {
    let golden = read("transcripts.json");
    let golden = golden["scenarios"].as_array().unwrap();
    let all = scenarios();
    assert_eq!(all.len(), golden.len(), "one golden block per scenario");
    let (mut equal, mut total, mut failures) = (0, 0, Vec::new());
    for ((file, name, entries), g) in all.iter().zip(golden) {
        assert_eq!(g["file"], file.as_str());
        assert_eq!(g["scenario"], name.as_str());
        let mine = ours(entries);
        let theirs = g["conversations"].as_array().unwrap();
        assert_eq!(mine.len(), theirs.len(), "{file} {name}: conversations");
        for (m, t) in mine.iter().zip(theirs) {
            total += 1;
            match first_difference("", t, m) {
                None => equal += 1,
                Some(d) => failures.push(format!("{file} {name} {}: {d}", t["conversation"])),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{equal} of {total} conversations equal; differences:\n{}",
        failures.join("\n")
    );
    assert_eq!(total, 146);
}

/// Each folded item carries the time its entry was made (`at`, seconds since 2001-01-01, as `created_at` reads), for
/// the phones' day dividers and "anything today" (kb PHN-627). The field is left out when serialised: the comparison
/// above, against FERB.app's own fold, stays byte for byte the Mac's (the named handling of this one addition).
// decision: PHN-696
#[test]
fn every_item_carries_its_entrys_time_and_the_golden_does_not_see_it() {
    let mut checked = 0;
    for (_, _, entries) in scenarios() {
        let made: std::collections::HashMap<&str, f64> = entries
            .iter()
            .map(|e| (e.id.as_str(), ferb_mobile::time::date(&e.created_at)))
            .collect();
        let t = fold::transcript(&entries, GOLDEN_UTC_OFFSET_S);
        for item in &t.items {
            let at = match item {
                fold::Item::Turn(i) => i.at,
                fold::Item::Reply(i) => i.at,
                fold::Item::Notice(i) | fold::Item::Reminder(i) => i.at,
            };
            if let Some(&when) = made.get(item.entry()) {
                assert!((at - when).abs() < 1e-3, "{}: {at} vs {when}", item.entry());
                checked += 1;
            }
            let v = serde_json::to_value(item).unwrap();
            assert!(v.get("at").is_none(), "`at` is never serialised: {v}");
        }
    }
    assert!(
        checked > 50,
        "only {checked} items had their entry in the scenario"
    );
}
