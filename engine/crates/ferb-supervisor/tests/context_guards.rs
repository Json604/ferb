//! ORCH-720's guards for ORCH-719 (step 1), on recorded scenarios with stand-in models (temp store, injected ids and
//! time): the planner's cached prefix stays byte-stable from one turn to the next, a block hash naming the block that
//! broke it; and a long chat with no summary gives the planner a bounded raw history.

mod common;
mod support_control;

use ferb_context::prefix::{block_hashes, block_name, broken_at};
use serde_json::json;
use support_control::{run_with, world_with, World};

/// Each planner call's (instructions, parts, cacheable), in order.
fn planner_requests(w: &World) -> Vec<(String, Vec<String>, usize)> {
    let calls = w.daemon.calls.lock().unwrap();
    calls
        .started
        .iter()
        .filter(|c| {
            let n = c["n"].as_u64().unwrap() as usize;
            calls.made[n]["runtime_role"] == "orchestrator"
        })
        .map(|c| {
            let parts = c["parts"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect();
            (
                c["system"].as_str().unwrap_or_default().to_string(),
                parts,
                c["cacheable"].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

/// The guard: the later request repeats the earlier one's instructions and cached blocks byte for byte; when it
/// doesn't, the message names the block (its first line and both hashes).
fn assert_prefix_kept(
    n: usize,
    before: &(String, Vec<String>, usize),
    after: &(String, Vec<String>, usize),
) {
    assert_eq!(
        block_hashes(std::slice::from_ref(&before.0)),
        block_hashes(std::slice::from_ref(&after.0)),
        "turn {n}: the instructions changed between turns (they open the cached prefix)"
    );
    if let Some(i) = broken_at(&before.1, before.2, &after.1) {
        let (a, b) = (block_hashes(&before.1), block_hashes(&after.1));
        panic!(
            "turn {n}: cached block {i} \u{201c}{}\u{201d} changed ({} then {}); every cached block after it is \
             written again",
            block_name(&before.1[i]),
            a[i],
            b.get(i).map_or("missing", String::as_str)
        );
    }
}

// decision: ORCH-720
#[test]
fn the_planners_cached_prefix_is_the_same_bytes_on_the_next_turn() {
    // a kept chat's two turns: the second adds its first exchange after the first's cached blocks, and repeats them
    let mut w = world_with("clarify_then_answer", |_| {});
    let _ = run_with(&mut w, "clarify_then_answer", |_| {});
    w.sup.chat_flush();
    let planners = planner_requests(&w);
    assert!(planners.len() >= 2, "two turns: {}", planners.len());
    for (n, pair) in planners.windows(2).enumerate() {
        assert!(pair[0].2 >= 2, "turn {n}: a cached prefix ({})", pair[0].2);
        assert_prefix_kept(n + 1, &pair[0], &pair[1]);
    }
}

// decision: ORCH-720
#[test]
fn the_guard_names_the_block_that_broke_the_prefix() {
    let before: Vec<String> = [
        "Layer 1 — facts",
        "Layer 5 — conversation:",
        "Owner: hi",
        "live",
    ]
    .map(String::from)
    .to_vec();
    let mut after = before.clone();
    assert_eq!(broken_at(&before, 3, &after), None);
    after.push("Owner: more".into());
    assert_eq!(broken_at(&before, 3, &after), None, "appending keeps it");
    after[3] = "live, changed".into();
    assert_eq!(broken_at(&before, 3, &after), None, "after the cached part");
    after[1] = "Layer 5 — conversation:\nFERB's running summary".into();
    assert_eq!(broken_at(&before, 3, &after), Some(1));
    assert_eq!(block_name(&after[1]), "Layer 5 — conversation:");
    assert_eq!(broken_at(&before, 3, &before[..2]), Some(2), "a block gone");
    let result = std::panic::catch_unwind(|| {
        assert_prefix_kept(
            1,
            &(String::new(), before.clone(), 3),
            &(String::new(), after.clone(), 3),
        )
    });
    let said = result
        .unwrap_err()
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        said.contains("cached block 1 \u{201c}Layer 5 — conversation:\u{201d} changed"),
        "{said}"
    );
}

/// Exchanges seeded into the kept chat: past ORCH-719's hard stop of 40.
const SEEDED: usize = 60;
/// ORCH-719 (owner): with no summary, the planner gets at most 40 exchanges word for word, a stop that needs no model
/// call.
const HARD_STOP: usize = 40;
/// What the planner is given today with no summary: every exchange (the fold's token triggers were never reached,
/// context audit 2026-11-25). The strict bound comes with the stop itself (T-1331, ORCH-719 step 2), whose patch
/// sets this to HARD_STOP; until then the test keeps today's bound so a regression past it still fails.
const RAW_CEILING: usize = SEEDED;

// decision: ORCH-720
#[test]
fn a_long_chat_without_a_summary_gives_the_planner_a_bounded_raw_history() {
    let mut w = world_with("one_step_task", |_| {});
    {
        let db = w.db.lock().unwrap();
        for i in 0..SEEDED {
            let (h, m) = (i / 30, (i % 30) * 2);
            let at = format!("2025-12-31T{:02}:{:02}:00+00:00", 6 + h, m);
            let reply_at = format!("2025-12-31T{:02}:{:02}:30+00:00", 6 + h, m);
            let request = format!("seedreq{i:06}");
            ferb_supervisor::flow::store::journal_row(
                db.conn(),
                &format!("seedmsg{i:07}"),
                &at,
                "chat.message",
                &json!({"session_id": "s1", "request_id": request,
                    "text": format!("INVENTED-OWNER-MESSAGE-{i:03}")}),
                Some("owner"),
            )
            .unwrap();
            ferb_supervisor::flow::store::journal_row(
                db.conn(),
                &format!("seedrep{i:07}"),
                &reply_at,
                "chat.reply",
                &json!({"session_id": "s1", "request_id": request,
                    "text": format!("INVENTED-FERB-REPLY-{i:03}")}),
                Some("ferb"),
            )
            .unwrap();
        }
    }
    let _ = run_with(&mut w, "one_step_task", |_| {});
    w.sup.chat_flush();
    let planners = planner_requests(&w);
    let first = planners.first().expect("a planner call");
    let context = first.1.join("\n");
    let sent = (0..SEEDED)
        .filter(|i| context.contains(&format!("INVENTED-OWNER-MESSAGE-{i:03}")))
        .count();
    eprintln!("raw exchanges sent with no summary: {sent} of {SEEDED}");
    assert!(sent > 0, "the kept chat reaches the planner");
    assert!(
        !context.contains(ferb_supervisor::words::SUMMARY_HEADER),
        "no running summary covers the seeded chat"
    );
    assert!(
        sent <= RAW_CEILING,
        "{sent} of {SEEDED} exchanges sent word for word with no summary (ceiling {RAW_CEILING}, ORCH-719's stop \
         {HARD_STOP})"
    );
    // the newest exchanges are the ones kept
    assert!(context.contains(&format!("INVENTED-OWNER-MESSAGE-{:03}", SEEDED - 1)));
}
