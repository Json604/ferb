//! The memory import's live card (MEM-456): the fold from a told run's progress events and the website watches'
//! records to the card every client draws. Words and numbers only; no store, no model.

use ferb_learning::import_live::{at_words, card, Days, RunNow, Watch};
use serde_json::{json, Value};

fn days() -> Days {
    Days {
        yesterday: "2026-11-21".into(),
        today: "2026-11-22".into(),
        tomorrow: "2026-11-23".into(),
    }
}

fn claude_code() -> RunNow {
    RunNow::new("r1", "claude_code", "Claude Code", true)
}

fn one_row(c: &Option<Value>) -> &Value {
    let rows = c.as_ref().expect("a card")["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    &rows[0]
}

/// The job's progress event as `import_job` publishes it.
fn reading(counts: Value) -> Value {
    json!({"run": "r1", "status": "running", "stage": "reading", "counts": counts})
}

// decision: MEM-455, MEM-456
#[test]
fn a_run_starts_without_a_bar_then_its_parts_fill_it() {
    let mut r = claude_code();
    let c = card(std::slice::from_ref(&r), &[], &days());
    assert_eq!(
        c.as_ref().unwrap()["title"],
        "Bringing in what Claude Code knows about you"
    );
    let row = one_row(&c);
    assert_eq!(row["key"], "run:r1");
    assert_eq!(row["stage"], "reading");
    assert_eq!(row["line"], "Looking through your sessions");
    assert_eq!(
        (row["done"].clone(), row["total"].clone()),
        (json!(0), json!(0))
    );
    assert_eq!(row["count"], "", "no bar before the plan is known");
    assert_eq!(row["found_line"], "");

    // the owner's live run of 2026-11-22: 33 of 40 parts read, 221 facts kept
    r.progress(&reading(json!({"conversations": 146, "messages": 2726, "chunks": 40, "chunks_done": 33,
                                "chunks_left": 7, "candidates": 347, "verified": 221, "facts_saved": 221,
                                "dropped": 126, "model_calls": 71, "model_calls_estimate": 80})));
    let c = card(std::slice::from_ref(&r), &[], &days());
    let row = one_row(&c);
    assert_eq!(row["line"], "Reading what you've typed");
    assert_eq!(
        (row["done"].clone(), row["total"].clone()),
        (json!(33), json!(40))
    );
    assert_eq!(row["count"], "33 of 40");
    assert_eq!(row["found_line"], "221 things found so far");
    assert_eq!(row["note"], "");

    // a resumed run: parts done before count as done (done + left), never model calls (86 of an estimated 80)
    r.progress(&reading(
        json!({"chunks": 40, "chunks_done": 39, "chunks_left": 1, "facts_saved": 274,
                                "model_calls": 86, "model_calls_estimate": 80}),
    ));
    let row = one_row(&card(std::slice::from_ref(&r), &[], &days())).clone();
    assert_eq!(row["count"], "39 of 40");
    assert_eq!(row["found_line"], "274 things found so far");
    let one = {
        let mut r = claude_code();
        r.progress(&reading(
            json!({"chunks_done": 0, "chunks_left": 2, "facts_saved": 1}),
        ));
        one_row(&card(&[r], &[], &days()))["found_line"].clone()
    };
    assert_eq!(one, "1 thing found so far");
}

// decision: MEM-456
#[test]
fn waiting_for_the_owners_turn_and_the_runs_end_keep_the_counts_and_a_gap_is_said_with_what_happens_next(
) {
    let mut r = claude_code();
    r.progress(&reading(
        json!({"chunks_done": 4, "chunks_left": 6, "facts_saved": 30}),
    ));
    // the wait for the owner's chat turn carries the counts as they stood
    r.progress(
        &json!({"run": "r1", "status": "running", "stage": "waiting for your current chat turn",
                       "counts": {"chunks_done": 4, "chunks_left": 6, "facts_saved": 30}}),
    );
    assert_eq!(
        one_row(&card(std::slice::from_ref(&r), &[], &days()))["count"],
        "4 of 10"
    );
    // the run's end (no stage) with a part left unread
    r.progress(&json!({"run": "r1", "status": "done",
                       "counts": {"chunks_done": 9, "chunks_left": 1, "facts_saved": 52, "failed_chunks": 1}}));
    let row = one_row(&card(std::slice::from_ref(&r), &[], &days())).clone();
    assert_eq!(row["count"], "9 of 10");
    assert_eq!(
        row["note"],
        "1 part couldn't be read just now. I'll go back to it next time."
    );
    let mut two = claude_code();
    two.progress(&reading(
        json!({"chunks_done": 1, "chunks_left": 0, "failed_chunks": 2}),
    ));
    assert_eq!(
        one_row(&card(&[two], &[], &days()))["note"],
        "2 parts couldn't be read just now. I'll go back to them next time."
    );
}

// decision: MEM-456
#[test]
fn placing_has_its_own_bar_from_batches_and_keeps_what_was_found() {
    let mut r = claude_code();
    r.progress(&reading(
        json!({"chunks_done": 40, "chunks_left": 0, "facts_saved": 274}),
    ));
    r.placing();
    let row = one_row(&card(std::slice::from_ref(&r), &[], &days())).clone();
    assert_eq!(row["stage"], "placing");
    assert_eq!(row["line"], "Putting together what I found");
    assert_eq!(
        row["detail"],
        "Many things come up more than once, so I'm joining them up."
    );
    assert_eq!(row["count"], "", "no bar before the first batch");
    assert_eq!(row["found_line"], "274 things found");
    r.progress(
        &json!({"run": "r1", "status": "running", "stage": "placing",
                       "counts": {"placed": 3, "to_place": 7}}),
    );
    // a wait while placing carries the batch call's own counts: the bar and what was found stay
    r.progress(
        &json!({"run": "r1", "status": "running", "stage": "waiting for your current chat turn",
                       "counts": {"model_calls": 1}}),
    );
    let row = one_row(&card(std::slice::from_ref(&r), &[], &days())).clone();
    assert_eq!(
        (row["done"].clone(), row["total"].clone()),
        (json!(3), json!(7))
    );
    assert_eq!(row["count"], "3 of 7");
    assert_eq!(row["found_line"], "274 things found");
}

fn watch(source: &str, name: &str) -> Watch {
    Watch {
        source: source.into(),
        name: name.into(),
        ..Watch::default()
    }
}

// decision: MEM-448, MEM-456
#[test]
fn a_websites_watch_says_its_stage_with_its_time() {
    let one = |w: Watch| one_row(&card(&[], &[w], &days())).clone();
    let mut w = watch("chatgpt", "ChatGPT");
    w.next_run = Some("2026-11-22T23:41".into());
    let row = one(w.clone());
    assert_eq!(row["key"], "site:chatgpt");
    assert_eq!(row["stage"], "asking");
    assert_eq!(
        row["line"],
        "I'll ask ChatGPT for a copy of your data at 23:41"
    );
    assert_eq!(row["count"], "", "a website's waiting stage has no bar");
    w.running = true;
    assert_eq!(
        one(w.clone())["line"],
        "Asking ChatGPT for a copy of your data"
    );
    w.running = false;
    w.asked_at = Some("2026-11-22T23:40".into());
    w.next_run = Some("2026-11-22T23:56".into());
    let row = one(w.clone());
    assert_eq!(row["stage"], "waiting_mail");
    assert_eq!(row["line"], "Waiting for ChatGPT's email — asked at 23:40");
    assert_eq!(
        row["detail"],
        "I look in your email for it every 12 hours. It can take a day or two."
    );
    w.running = true;
    let row = one(w.clone());
    assert_eq!(row["stage"], "checking_mail");
    assert_eq!(row["line"], "Checking your email for ChatGPT's copy");
    assert_eq!(row["detail"], "Asked at 23:40");
    // a try that needed the owner: said with when the next one is
    w.running = false;
    w.trouble = true;
    w.next_run = Some("2026-11-23T00:25".into());
    assert_eq!(
        one(w.clone())["note"],
        "The last try didn't go through. I'll try again tomorrow at 00:25."
    );
    let mut first = watch("claude", "Claude");
    first.trouble = true;
    first.next_run = Some("2026-11-22T23:56".into());
    let row = one(first);
    assert_eq!(row["line"], "Asking Claude for a copy of your data");
    assert_eq!(
        row["note"],
        "The last try didn't go through. I'll try again at 23:56."
    );
}

// decision: MEM-456
#[test]
fn times_are_said_as_today_yesterday_or_a_date() {
    assert_eq!(at_words("2026-11-22T23:40", &days()), "at 23:40");
    assert_eq!(at_words("2026-11-21T08:05", &days()), "yesterday at 08:05");
    assert_eq!(at_words("2026-11-23T00:25", &days()), "tomorrow at 00:25");
    assert_eq!(at_words("2026-11-03T08:05", &days()), "on Nov 3 at 08:05");
}

// decision: MEM-456
#[test]
fn two_sources_share_one_card_and_a_run_takes_its_watchs_place() {
    let watches = [watch("chatgpt", "ChatGPT"), watch("claude", "Claude")];
    let c = card(&[], &watches, &days()).unwrap();
    assert_eq!(
        c["title"],
        "Bringing in what ChatGPT and Claude know about you"
    );
    assert_eq!(c["rows"].as_array().unwrap().len(), 2);
    // ChatGPT's export came in: its run's bar shows instead of its watch
    let mut run = RunNow::new("r9", "chatgpt", "ChatGPT", false);
    run.progress(
        &json!({"run": "r9", "status": "running", "stage": "reading",
                         "counts": {"chunks_done": 2, "chunks_left": 10, "facts_saved": 12}}),
    );
    let c = card(&[run], &watches, &days()).unwrap();
    let rows = c["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["key"], "run:r9");
    assert_eq!(rows[0]["line"], "Reading your chats");
    assert_eq!(rows[1]["key"], "site:claude");
    assert_eq!(
        c["title"],
        "Bringing in what ChatGPT and Claude know about you"
    );
    // nothing under way: no card
    assert_eq!(card(&[], &[], &days()), None);
}

// decision: MEM-456 — owner-facing words are plain and never say "AI"
#[test]
fn no_word_on_the_card_says_ai() {
    let mut r = claude_code();
    r.progress(&reading(
        json!({"chunks_done": 1, "chunks_left": 1, "facts_saved": 3, "failed_chunks": 1}),
    ));
    let mut p = RunNow::new("r2", "codex", "Codex", true);
    p.placing();
    let mut w = watch("claude", "Claude");
    w.asked_at = Some("2026-11-20T10:00".into());
    w.trouble = true;
    w.next_run = Some("2026-11-22T10:15".into());
    let text = card(&[r, p], &[w, watch("chatgpt", "ChatGPT")], &days())
        .unwrap()
        .to_string();
    assert!(!text.contains(" AI") && !text.contains("AI "), "{text}");
}
