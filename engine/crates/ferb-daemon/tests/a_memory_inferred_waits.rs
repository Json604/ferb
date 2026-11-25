//! The upgrade that takes FERB's understanding out of always on until the owner's Yes (MEM-480, his "idk ig wait" of
//! 2026-11-25; MEM-482), on a store shaped like his: 65 imported lines approved at once by one Approve all, his own
//! words and one Mac-scan fact in the profile, plus lines an earlier build put there by itself. After the start his
//! words, the observed fact and the 65 he approved stay always on (his "yeah keep them", MEM-483); the lines with no
//! answer of his leave it and wait in the review (the chat's card, the Memory page's "To check" through
//! `memory.review`), remembered and found by lookups; his answers work as before. Temporary folders only; no model.

mod a_support;

use a_support::{Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::{candidates, memory, memory_import, State};
use serde_json::{json, Value};

const LINES: usize = 65;
/// Lines an earlier build put in always on by itself, never answered.
const JOINED: usize = 3;

async fn daemon(tmp: &std::path::Path) -> Daemon {
    Daemon::start(tmp, "01", "id", FakeClock::new(None), Seq::new("id")).await
}

async fn call(d: &Daemon, method: &str, params: Value) -> Value {
    let mut raw = Raw::connect(d.server.path()).await;
    raw.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    raw.recv().await;
    raw.send(&json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}).to_string())
        .await;
    serde_json::from_str(&raw.recv().await).unwrap()
}

async fn setup_done(d: &Daemon) {
    d.on(|st| {
        Box::pin(async move {
            st.journal("setup.finished", json!({"done": true}), None)
                .unwrap();
        })
    })
    .await;
}

async fn daily(d: &Daemon) {
    d.on(|st| Box::pin(async move { memory_import::daily(&st, "mac") }))
        .await;
}

fn asks(d: &Daemon) -> Vec<Value> {
    d.stand
        .calls
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.get("ask").cloned())
        .collect()
}

fn save(st: &State, kind: &str, key: &str, content: &str, origin: &str, source: &str) -> String {
    st.memory
        .save(
            kind,
            "owner",
            key,
            content,
            origin,
            &[("journal".into(), source.to_owned())],
            0.6,
            &[],
            "",
            Some(origin == "owner_stated"),
        )
        .unwrap()
        .item
        .id
}

/// Everything the prompts carry as always on: the settled block and what joined since.
fn always_on(st: &State) -> String {
    memory::core_profile_text(st) + &memory::core_profile_pending(st)
}

/// The owner's store before this build: what he said, what the Mac scan saw, and the imported lines an earlier build
/// put in always on, then all approved by one Approve all in the review (23 Nov, 03:40:49).
fn owners_store(st: &std::rc::Rc<State>) -> (String, String, Vec<String>, Vec<String>) {
    // his first start's one-time clean-up ran on an empty memory (17 Nov): no older build's Keep answers
    ferb_learning::upgrade::clean_before_tier1(&st.memory, &|kind: &str, data: Value| {
        st.journal.append(kind, Some(data), None).map(|_| ())
    })
    .unwrap();
    let said = st.journal("chat.message", json!({}), None).unwrap();
    let his = save(
        st,
        "fact",
        "legal_name",
        "My legal name is Sam",
        "owner_stated",
        &said.id,
    );
    let scan = st
        .journal(ferb_learning::items::MAC_PROFILE_KIND, json!({}), None)
        .unwrap();
    let seen = save(
        st,
        "fact",
        "main_activity",
        "You seem to use this Mac mainly for writing.",
        "observed",
        &scan.id,
    );
    let import = st
        .journal(
            ferb_learning::import_job::SOURCE_KIND,
            json!({"run": "r1"}),
            None,
        )
        .unwrap();
    let day = ferb_clock::py::iso(&st.now())[..10].to_owned();
    let mut lines = Vec::new();
    for n in 0..LINES {
        let content = format!("The owner wants step {n} planned before it is built");
        let id = save(
            st,
            "preference",
            &format!("rule_{n}"),
            &content,
            "inferred",
            &import.id,
        );
        let row = json!({"you": content.replace("The owner", "You"), "offer": "", "said": day, "first_said": day,
                         "days": 1, "sources": ["claude_code"], "ended": false, "until": "", "review_on": "",
                         "use": "follow", "holds_up": false, "always_on": "suggested"});
        st.store
            .exec(
                "INSERT INTO memory_import_lines (id, item_id, kind, subject, key, data_json, created_at, updated_at) \
                 VALUES (?, ?, 'preference', 'owner', ?, ?, ?, ?)",
                ferb_learning::sql![format!("l{n}"), id, format!("rule_{n}"), row.to_string(), day, day],
            )
            .unwrap();
        lines.push(id);
    }
    for id in [&his, &seen] {
        st.profile.add(id, "Who you are", "pass").unwrap();
    }
    // his one Approve all: each line his pin, its line approved
    let n = memory_import::job(st)
        .answer_review(&st.profile, "always_on", &lines, "approve")
        .unwrap();
    assert_eq!(n, LINES);
    // and what an earlier build joined by itself (MEM-445), no answer of his on it
    let mut joined = Vec::new();
    for n in 0..JOINED {
        let content = format!("The owner likes theme {n} in editors");
        let id = save(
            st,
            "preference",
            &format!("theme_{n}"),
            &content,
            "inferred",
            &import.id,
        );
        let row = json!({"you": content.replace("The owner", "You"), "offer": "", "said": day, "first_said": day,
                         "days": 1, "sources": ["claude_code"], "ended": false, "until": "", "review_on": "",
                         "use": "follow", "holds_up": false});
        st.store
            .exec(
                "INSERT INTO memory_import_lines (id, item_id, kind, subject, key, data_json, created_at, updated_at) \
                 VALUES (?, ?, 'preference', 'owner', ?, ?, ?, ?)",
                ferb_learning::sql![format!("j{n}"), id, format!("theme_{n}"), row.to_string(), day, day],
            )
            .unwrap();
        st.store
            .exec(
                "INSERT INTO memory_profile (item_id, section, position, added_by, added_at) \
                 VALUES (?, 'Recently said', 99, 'code', 'x')",
                ferb_learning::sql![id],
            )
            .unwrap();
        joined.push(id);
    }
    st.profile.settle("start").unwrap();
    (his, seen, lines, joined)
}

// decision: MEM-480, MEM-482, MEM-483
#[tokio::test]
async fn the_upgrade_keeps_what_he_approved_and_asks_about_what_an_earlier_build_joined() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let (his, seen, lines, joined) = d
        .on(|st| {
            Box::pin(async move {
                let out = owners_store(&st);
                // the earlier build's rows (the guard already keeps the 3 unanswered ones out of the prompts)
                let rows = st.profile.ids().unwrap();
                assert!(out.2.iter().chain(&out.3).all(|id| rows.contains(id)));
                let before = always_on(&st);
                assert!(!out.3.iter().any(|id| before.contains(id)), "{before}");
                memory::start(&st).unwrap();
                out
            })
        })
        .await;
    let (h, s, l, j) = (his.clone(), seen.clone(), lines.clone(), joined.clone());
    d.on(move |st| {
        Box::pin(async move {
            // his words, the observed fact and the 65 he approved stay always on; what no one answered leaves
            let text = always_on(&st);
            assert!(text.contains(&h) && text.contains(&s), "{text}");
            assert!(
                l.iter().all(|id| text.contains(id)),
                "his Approve all stands: {text}"
            );
            assert!(!j.iter().any(|id| text.contains(id)), "{text}");
            assert_eq!(st.profile.entries().unwrap().len(), 2 + LINES);
            // nothing deleted: what left is still remembered and found by the lookups' search
            for id in &j {
                assert_eq!(st.memory.get(id).unwrap().unwrap().status, "active");
            }
            let found = candidates::memory_item_search(&st, "theme 1 in editors").unwrap();
            assert!(found.contains(&j[1]), "{found:?}");
        })
    })
    .await;
    // nothing asked at the start (MEM-485); the day's first open: one card in the chat for what waits, as the chat,
    // the Memory page and the phones draw it
    assert!(asks(&d).is_empty());
    setup_done(&d).await;
    daily(&d).await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["kind"], "memory_review");
    assert_eq!(
        asked[0]["detail"],
        "3 things to keep in mind all the time. Untick anything that isn't right, or just tell me."
    );
    // the Memory page's "To check" reads the same list, any time
    let review = call(&d, "memory.review", json!({})).await;
    assert_eq!(review["result"]["count"], json!(JOINED), "{review}");
    let part = &review["result"]["parts"][0];
    assert_eq!(part["title"], "Keep these in mind all the time?");
    let labels: Vec<&str> = part["all"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, vec!["Approve all", "Decline all"]);
    // a second start (a restart) asks the same question again with the list as it stands, never a new one
    d.on(|st| Box::pin(async move { memory::start(&st).unwrap() }))
        .await;
    let again = asks(&d);
    assert!(again.iter().all(|a| a["id"] == asked[0]["id"]), "{again:?}");
    // his Yes puts one in; his No keeps another out, remembered
    let reply = call(
        &d,
        "memory.review.answer",
        json!({"part": "always_on", "item_ids": [joined[0]], "answer": "approve"}),
    )
    .await;
    assert_eq!(reply["result"]["answered"], json!(1), "{reply}");
    call(
        &d,
        "memory.review.answer",
        json!({"part": "always_on", "item_ids": [joined[1]], "answer": "decline"}),
    )
    .await;
    let (l, j) = (lines.clone(), joined.clone());
    d.on(move |st| {
        Box::pin(async move {
            let text = always_on(&st);
            assert!(text.contains(&j[0]) && !text.contains(&j[1]), "{text}");
            assert_eq!(st.memory.get(&j[1]).unwrap().unwrap().status, "active");
            assert_eq!(
                memory_import::job(&st).review().unwrap()["count"],
                json!(JOINED - 2)
            );
            let rows = st
                .store
                .count(
                    "SELECT COUNT(*) FROM journal WHERE kind = ?",
                    ferb_learning::sql![ferb_learning::import_review::WAITING_KIND],
                )
                .unwrap();
            assert_eq!(rows, 1, "journaled only when it moved something");
            // a later start keeps every answer, the old Approve all and the new ones
            memory::start(&st).unwrap();
            let text = always_on(&st);
            assert!(text.contains(&j[0]) && !text.contains(&j[1]), "{text}");
            assert!(l.iter().all(|id| text.contains(id)), "{text}");
        })
    })
    .await;
}

// decision: MEM-481, MEM-485 — FERB's own understanding seen on two days waits in the same list, the day's first
// open asks it, and Not always on on the Memory page is his No
#[tokio::test]
async fn ferbs_own_understanding_is_suggested_and_the_memory_pages_no_keeps_it_out() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    setup_done(&d).await;
    let id = d
        .on(|st| {
            Box::pin(async move {
                let first = st.journal("chat.message", json!({}), None).unwrap();
                save(
                    &st,
                    "preference",
                    "drink",
                    "The owner prefers tea",
                    "inferred",
                    &first.id,
                )
            })
        })
        .await;
    // said again three days later
    d.clock.advance(3.0 * 86_400.0);
    let id = d
        .on(move |st| {
            Box::pin(async move {
                let again = st.journal("chat.message", json!({}), None).unwrap();
                save(
                    &st,
                    "preference",
                    "drink",
                    "The owner prefers tea",
                    "inferred",
                    &again.id,
                );
                assert!(!always_on(&st).contains(&id), "never by itself");
                memory_import::hourly(&st).await;
                id
            })
        })
        .await;
    assert!(asks(&d).is_empty(), "the hourly pass asks nothing");
    daily(&d).await;
    let asked = asks(&d);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(
        asked[0]["detail"],
        "1 thing to keep in mind all the time. Untick anything that isn't right, or just tell me."
    );
    let review = call(&d, "memory.review", json!({})).await;
    let item = &review["result"]["parts"][0]["groups"][0];
    assert_eq!(item["title"], "How you like me to work");
    assert_eq!(item["items"][0]["text"], "The owner prefers tea");
    // Not always on on the Memory page: his No, the question closes, never suggested again
    let reply = call(&d, "memory.profile", json!({"op": "remove", "id": id})).await;
    assert!(reply.get("error").is_none(), "{reply}");
    let i = id.clone();
    d.on(move |st| {
        Box::pin(async move {
            assert_eq!(memory_import::job(&st).review().unwrap()["count"], json!(0));
            assert!(memory::pending_own(&st, memory_import::REVIEW_KIND).is_none());
            memory_import::hourly(&st).await;
            assert!(!always_on(&st).contains(&i));
            assert_eq!(st.memory.get(&i).unwrap().unwrap().status, "active");
        })
    })
    .await;
    assert_eq!(asks(&d).len(), 1);
}
