//! The daily memory review (MEM-485, the owner's "this one tick for every question is toooooo tedious" of 2026-11-25;
//! MEM-486): nothing is asked when something is learned; the first time he opens FERB each day (FERB.app or a phone),
//! on this Mac's calendar, code counts what waits: one to five are one card with all of them, more are one note whose
//! button opens the list; the card's Approve all answers every line, the unticked ones declined. Nothing expires and
//! the note changes at most once a day. Temporary folders only; no model (a stand-in counts any call).

mod a_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a_support::{Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_clock::zone::Zone;
use ferb_daemon::areas_a::{memory, memory_import, State};
use serde_json::{json, Value};

const DAY: f64 = 86_400.0;

/// A daemon on `start` (UTC) whose Mac is in `zone`, with a model stand-in that only counts calls.
async fn daemon(tmp: &std::path::Path, start: &str, zone: &str) -> (Daemon, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let clock = FakeClock::new(Some(ferb_clock::py::parse(start).unwrap()))
        .with_zone(Zone::named(zone).unwrap());
    let d = Daemon::start_with(
        tmp,
        "01",
        "id",
        clock,
        Seq::new("id"),
        Some(Arc::new(move |_, _, _, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(ferb_learning::Error::Runtime(
                "no model in this test".into(),
            ))
        })),
    )
    .await;
    // the gateway keeps a question until it is answered or closed
    *d.stand.keeps.lock().unwrap() = Some(Default::default());
    d.on(|st| {
        Box::pin(async move {
            st.journal("setup.finished", json!({"done": true}), None)
                .unwrap();
        })
    })
    .await;
    (d, calls)
}

async fn call(d: &Daemon, method: &str, params: Value) -> Value {
    let mut raw = Raw::connect(d.server.path()).await;
    raw.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    raw.recv().await;
    raw.send(&json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}).to_string())
        .await;
    let reply: Value = serde_json::from_str(&raw.recv().await).unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    reply["result"].clone()
}

async fn opened(d: &Daemon, client: &str) -> Value {
    call(d, "memory.review.daily", json!({"client": client})).await
}

fn of(d: &Daemon, what: &str) -> Vec<Value> {
    d.stand
        .calls
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.get(what).cloned())
        .collect()
}

/// `n` remembered facts whose check date is today (MEM-472): each waits in "Are these still true?".
async fn due(d: &Daemon, n: usize, tag: &str) -> Vec<String> {
    let tag = tag.to_owned();
    d.on(move |st| {
        Box::pin(async move {
            (0..n)
                .map(|i| {
                    let item = st
                        .memory
                        .save(
                            "fact",
                            "owner",
                            &format!("trip {tag} {i}"),
                            &format!("The owner is away for trip {tag} {i}"),
                            "inferred",
                            &[("journal".into(), format!("j-{tag}-{i}"))],
                            0.6,
                            &[],
                            "",
                            Some(false),
                        )
                        .unwrap()
                        .item;
                    let now = st.memory.clock.now();
                    st.memory.set_review(&item.id, Some(&now)).unwrap();
                    item.id
                })
                .collect()
        })
    })
    .await
}

fn rows(st: &State) -> Vec<Value> {
    st.store
        .query(
            "SELECT data_json FROM journal WHERE kind = ? ORDER BY seq",
            ferb_learning::sql![memory_import::DAILY_KIND],
        )
        .unwrap()
        .iter()
        .map(|r| serde_json::from_str(&r.s_at(0)).unwrap())
        .collect()
}

async fn daily_rows(d: &Daemon) -> Vec<Value> {
    d.on(|st| Box::pin(async move { rows(&st) })).await
}

// decision: MEM-485, MEM-486 — once a day, at the first open, on the Mac's own calendar (his time zone), not UTC's
#[tokio::test]
async fn the_first_open_of_each_day_checks_once_on_the_macs_calendar() {
    let tmp = tempfile::tempdir().unwrap();
    // 15:30 on 25 Nov in India (UTC+5:30)
    let (d, models) = daemon(tmp.path(), "2026-11-25T10:00:00+00:00", "Asia/Kolkata").await;
    due(&d, 1, "a").await;
    let first = opened(&d, "mac").await;
    assert_eq!(
        first,
        json!({"day": "2026-11-25", "checked": true, "count": 1, "shows": "card"})
    );
    assert_eq!(of(&d, "ask").len(), 1);
    // every later open that day does nothing
    d.clock.advance(3.0 * 3600.0);
    assert_eq!(
        opened(&d, "mac").await,
        json!({"day": "2026-11-25", "checked": false})
    );
    assert_eq!(daily_rows(&d).await.len(), 1);
    // 18:45 UTC is still 25 Nov in UTC but 00:15 on the 26th on his Mac: a new day
    d.clock
        .set_wall(ferb_clock::py::parse("2026-11-25T18:45:00+00:00").unwrap());
    assert_eq!(opened(&d, "mac").await["day"], "2026-11-26");
    // 01:00 UTC on the 26th is a new day in UTC, the same day on his Mac (06:30): nothing
    d.clock
        .set_wall(ferb_clock::py::parse("2026-11-26T01:00:00+00:00").unwrap());
    assert_eq!(opened(&d, "mac").await["checked"], json!(false));
    let r = daily_rows(&d).await;
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[1]["day"], "2026-11-26");
    // the card that waits is not asked twice; counting and asking took no model
    assert_eq!(of(&d, "ask").len(), 1);
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

// decision: MEM-485 — a phone opened first counts for the day; FERB.app's open later that day does nothing
#[tokio::test]
async fn a_phone_opened_first_counts_for_the_day() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, _) = daemon(tmp.path(), "2026-11-25T07:00:00+00:00", "Europe/London").await;
    due(&d, 2, "a").await;
    assert_eq!(opened(&d, "android").await["shows"], "card");
    assert_eq!(opened(&d, "mac").await["checked"], json!(false));
    assert_eq!(opened(&d, "iphone").await["checked"], json!(false));
    let r = daily_rows(&d).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["client"], "android");
    assert_eq!(of(&d, "ask").len(), 1);
}

// decision: MEM-485 — nothing is shown before setup is done, nor when nothing waits
#[tokio::test]
async fn nothing_waiting_shows_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, _) = daemon(tmp.path(), "2026-11-25T07:00:00+00:00", "UTC").await;
    let r = opened(&d, "mac").await;
    assert_eq!(r["shows"], "nothing");
    assert!(of(&d, "ask").is_empty() && of(&d, "tell_owner").is_empty());
}

// decision: MEM-485 — five or fewer: one card with all of them; six or more: no card, one note whose button opens the
// list
#[tokio::test]
async fn five_is_a_card_and_six_is_a_note_that_opens_the_list() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, models) = daemon(tmp.path(), "2026-11-25T07:00:00+00:00", "UTC").await;
    due(&d, 5, "a").await;
    assert_eq!(opened(&d, "mac").await["shows"], "card");
    let asked = of(&d, "ask");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["kind"], "memory_review");
    assert_eq!(
        asked[0]["summary"],
        "Can you check a few things I learned about you?"
    );
    assert!(of(&d, "tell_owner").is_empty());
    // the card shows them all: the list it draws holds the five
    let review = call(&d, "memory.review", json!({})).await;
    assert_eq!(review["count"], json!(5));
    // a sixth learned that day waits for the next day's first open
    due(&d, 1, "b").await;
    assert_eq!(opened(&d, "mac").await["checked"], json!(false));
    d.clock.advance(DAY);
    assert_eq!(
        opened(&d, "mac").await,
        json!({"day": "2026-11-26", "checked": true, "count": 6, "shows": "note"})
    );
    // the card goes (closed, never answered for him) and one note says how many, its button opening the list
    assert_eq!(of(&d, "supersede"), vec![asked[0]["id"].clone()]);
    assert_eq!(of(&d, "ask").len(), 1, "no card for six");
    let told = of(&d, "tell_owner");
    assert_eq!(told.len(), 1);
    assert_eq!(
        told[0]["text"],
        "I learned 6 things about you. Review them when you like."
    );
    assert_eq!(
        told[0]["cards"],
        json!([{"type": "page", "page": "review", "line": "Review"}])
    );
    assert_eq!(told[0]["origin"], "ferb");
    // the list it opens is the review, the same as the Memory page's To check
    assert_eq!(
        call(&d, "memory.review", json!({})).await["count"],
        json!(6)
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

// decision: MEM-485 — nothing expires and nothing nags: the same number says nothing new, a new number takes the old
// note's place at most once a day, and nothing left folds it away
#[tokio::test]
async fn the_note_changes_at_most_once_a_day_and_nothing_expires() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, _) = daemon(tmp.path(), "2026-11-25T07:00:00+00:00", "UTC").await;
    let first = due(&d, 6, "a").await;
    assert_eq!(opened(&d, "mac").await["shows"], "note");
    let note = of(&d, "tell_owner")[0]["ident"].clone();
    // the same six, day after day, for forty days: no new note, nothing gone from the list
    for _ in 0..40 {
        d.clock.advance(DAY);
        assert_eq!(opened(&d, "mac").await["count"], json!(6));
    }
    assert_eq!(of(&d, "tell_owner").len(), 1);
    assert!(of(&d, "fold_notices")
        .iter()
        .all(|f| !f["refs"].as_array().unwrap().contains(&note)));
    let r = daily_rows(&d).await;
    assert_eq!(r.last().unwrap()["note"], note);
    // a seventh: the next day's note says 7 and the old one is folded (kept, never drawn again)
    let more = due(&d, 1, "b").await;
    d.clock.advance(DAY);
    opened(&d, "mac").await;
    let told = of(&d, "tell_owner");
    assert_eq!(told.len(), 2);
    assert_eq!(
        told[1]["text"],
        "I learned 7 things about you. Review them when you like."
    );
    let folds = of(&d, "fold_notices");
    let last = folds.last().unwrap();
    assert_eq!(last["refs"], json!([note]));
    assert_eq!(last["into"], "memory-review");
    // an eighth the same day: no change until tomorrow
    due(&d, 1, "c").await;
    opened(&d, "android").await;
    assert_eq!(of(&d, "tell_owner").len(), 2);
    // he answers everything on the list: the next day's first open folds the note and says nothing
    let all: Vec<String> = first.into_iter().chain(more).collect();
    let left = call(
        &d,
        "memory.review.all",
        json!({"approve": all, "decline": []}),
    )
    .await;
    assert_eq!(left["count"], json!(1));
    let second = told[1]["ident"].clone();
    d.clock.advance(DAY);
    assert_eq!(opened(&d, "mac").await["shows"], "card");
    let folds = of(&d, "fold_notices");
    assert_eq!(folds.last().unwrap()["refs"], json!([second]));
    assert_eq!(of(&d, "tell_owner").len(), 2);
}

/// One of FERB's own understanding (a preference said on two separate days, MEM-481): waits in "Keep these in mind all
/// the time?".
async fn understood(d: &Daemon) -> String {
    let mut id = String::new();
    for day in 0..2 {
        if day > 0 {
            d.clock.advance(DAY);
        }
        id = d
            .on(|st| {
                Box::pin(async move {
                    let row = st.journal("chat.message", json!({}), None).unwrap();
                    st.memory
                        .save(
                            "preference",
                            "owner",
                            "drink",
                            "The owner prefers tea",
                            "inferred",
                            &[("journal".into(), row.id)],
                            0.6,
                            &[],
                            "",
                            Some(false),
                        )
                        .unwrap()
                        .item
                        .id
                })
            })
            .await;
    }
    id
}

// decision: MEM-488 — the card's Approve all answers every line it shows: the ticked ones get their part's yes, the
// unticked ones its no, in one call, and the card closes when nothing is left
#[tokio::test]
async fn approve_all_with_exceptions_answers_every_line() {
    let tmp = tempfile::tempdir().unwrap();
    let (d, models) = daemon(tmp.path(), "2026-11-25T07:00:00+00:00", "UTC").await;
    let tea = understood(&d).await;
    let facts = due(&d, 2, "a").await;
    assert_eq!(opened(&d, "mac").await["shows"], "card");
    let asked = of(&d, "ask");
    // the list's own buttons, at its top
    let review = call(&d, "memory.review", json!({})).await;
    assert_eq!(review["count"], json!(3));
    assert_eq!(
        review["all"],
        json!([{"answer": "approve", "label": "Approve all"},
               {"answer": "decline", "label": "Decline all"}])
    );
    assert_eq!(review["some"], "Approve {n}");
    assert_eq!(review["hint"], "Untick anything that isn't right.");
    assert_eq!(review["parts"][0]["part"], "always_on");
    assert_eq!(review["parts"][1]["part"], "still_true");
    d.published.lock().unwrap().clear();
    // he unticks the second fact and presses Approve 2
    let reply = call(
        &d,
        "memory.review.all",
        json!({"approve": [tea, facts[0]], "decline": [facts[1]]}),
    )
    .await;
    assert_eq!(reply["answered"], json!(3));
    assert_eq!(reply["count"], json!(0));
    let (t, f) = (tea.clone(), facts.clone());
    d.on(move |st| {
        Box::pin(async move {
            // approved: always on as his; still true: kept, its check date cleared
            assert!(st.profile.entries().unwrap().iter().any(|e| e.item.id == t));
            let kept = st.memory.get(&f[0]).unwrap().unwrap();
            assert!(kept.review_at.is_none() && kept.valid_to.is_none());
            // the unticked one: "No, not any more", so it is no longer used, and kept
            let ended = st.memory.get(&f[1]).unwrap().unwrap();
            assert!(
                ended.valid_to.is_some() || ended.status != "active",
                "{ended:?}"
            );
            assert!(memory::pending_own(&st, memory_import::REVIEW_KIND).is_none());
        })
    })
    .await;
    let resolved: Vec<Value> = d
        .published
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["topic"] == "approval.resolved")
        .map(|e| e["data"].clone())
        .collect();
    assert_eq!(
        resolved,
        vec![json!({"approval_id": asked[0]["id"], "state": "superseded"})]
    );
    // Decline all on another day's card: every line gets its part's no
    d.clock.advance(DAY);
    let more = due(&d, 1, "b").await;
    assert_eq!(opened(&d, "mac").await["shows"], "card");
    let reply = call(
        &d,
        "memory.review.all",
        json!({"approve": [], "decline": more}),
    )
    .await;
    assert_eq!(reply["count"], json!(0));
    assert_eq!(models.load(Ordering::SeqCst), 0);
}
