//! The memory import's live card on group A (MEM-456): published on `memory.import.live` from the start of an import
//! the owner is told about until its one notice, in `memory.import.status` while it goes, and the website watches'
//! stages from their records. Temporary folders only; no model, browser or mail.

mod a_support;

use std::rc::Rc;

use a_support::{Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::{memory_import, schedules, State};
use serde_json::{json, Map, Value};

async fn daemon(tmp: &std::path::Path) -> Daemon {
    Daemon::start(tmp, "01", "id", FakeClock::new(None), Seq::new("id")).await
}

/// Every card published, in order (None: the card left).
fn cards(d: &Daemon) -> Vec<Option<Value>> {
    d.published
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["topic"] == memory_import::LIVE_TOPIC)
        .map(|e| Some(e["data"]["card"].clone()).filter(|c| c.is_object()))
        .collect()
}

fn told(d: &Daemon) -> Vec<Value> {
    d.stand
        .calls
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.get("tell_owner").cloned())
        .collect()
}

async fn status(d: &Daemon) -> Value {
    let mut raw = Raw::connect(d.server.path()).await;
    raw.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    raw.recv().await;
    raw.send(r#"{"jsonrpc": "2.0", "id": 2, "method": "memory.import.status", "params": {}}"#)
        .await;
    let reply: Value = serde_json::from_str(&raw.recv().await).unwrap();
    assert!(reply["result"].is_object(), "{reply}");
    reply["result"].clone()
}

// decision: MEM-455, MEM-456, MEM-445
#[tokio::test]
async fn a_told_import_shows_its_card_until_its_one_notice_says_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    // one line typed into Claude Code eleven days before the clock's now (2026-01-01 09:00 UTC)
    let history = d.user.join(".claude/history.jsonl");
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    std::fs::write(
        &history,
        "{\"display\": \"I keep bees on my roof in Lisbon\", \"timestamp\": 1766224800000, \"sessionId\": \"s1\", \
         \"project\": \"/Users/me/hive\"}\n",
    )
    .unwrap();
    assert_eq!(status(&d).await.get("live"), None, "nothing under way");
    let said = d
        .on(|st| Box::pin(async move { memory_import::local_answered(&st, true) }))
        .await;
    assert!(
        said.starts_with("Thanks! I'm reading through Claude Code"),
        "{said}"
    );
    // the run goes on in the background (no model here: its one part can't be read)
    for _ in 0..400 {
        if cards(&d).len() > 1 && cards(&d).last() == Some(&None) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let shown = cards(&d);
    assert!(shown.len() >= 3, "{shown:?}");
    let first = shown[0].as_ref().expect("the card shows as the run starts");
    assert_eq!(
        first["title"],
        "Bringing in what Claude Code knows about you"
    );
    assert_eq!(first["rows"][0]["source"], "claude_code");
    assert_eq!(first["rows"][0]["line"], "Looking through your sessions");
    let read = shown
        .iter()
        .flatten()
        .find(|c| c["rows"][0]["total"] == json!(1))
        .expect("its parts, once planned, make the bar");
    assert_eq!(read["rows"][0]["count"], "0 of 1");
    assert!(
        shown
            .iter()
            .flatten()
            .any(|c| c["rows"][0]["stage"] == "placing"
                && c["rows"][0]["note"]
                    == "1 part couldn't be read just now. I'll go back to it next time."),
        "placing, with the part left unread said: {shown:?}"
    );
    assert_eq!(shown.last(), Some(&None), "the card leaves at the end");
    // its end is the one notice, never a second message
    let notices = told(&d);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0]["ident"]
            .as_str()
            .unwrap()
            .starts_with("memory-import-"),
        "{notices:?}"
    );
    assert!(
        notices[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("I couldn't finish bringing in what Claude Code knows about you"),
        "{notices:?}"
    );
    assert_eq!(status(&d).await.get("live"), None);
    // the hourly pass's quiet updates never show a card
    let before = cards(&d).len();
    d.on(|st| {
        Box::pin(async move { memory_import::local_update(&st, memory_import::Tell::Never).await })
    })
    .await;
    assert_eq!(cards(&d).len(), before, "{:?}", cards(&d));
}

fn ended(
    sid: String,
    status: &'static str,
) -> impl FnOnce(Rc<State>) -> ferb_daemon::areas_a::LocalFuture<()> {
    move |st: Rc<State>| {
        Box::pin(async move {
            let s = st.schedules().get(&sid).unwrap().unwrap();
            let mut f = Map::new();
            f.insert("status".into(), json!(status));
            f.insert("answer".into(), json!("asked for the export"));
            f.insert("task_id".into(), json!("t1"));
            f.insert("watch_met".into(), json!(false));
            let now = st.now();
            schedules::schedule_finished(&st, &s, f, 0.0, true, &now)
                .await
                .unwrap();
            memory_import::watch_moved(&st, &s);
        })
    }
}

// decision: MEM-448, MEM-451, MEM-456
#[tokio::test]
async fn the_website_watches_show_their_stage_from_their_records() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let made = d
        .on(|st| {
            Box::pin(async move {
                let ident = json!({"options": {"Chrome “Work”": "Chrome/Profile 1"}}).to_string();
                memory_import::sites_answered(&st, "granted", "Chrome “Work”", &ident);
                st.schedules().all(&["active"]).unwrap()
            })
        })
        .await;
    assert_eq!(made.len(), 2);
    let card = cards(&d).last().cloned().flatten().expect("a card at once");
    assert_eq!(
        card["title"],
        "Bringing in what ChatGPT and Claude know about you"
    );
    let rows = card["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    // the clock's zone is UTC: the first run a minute after the choice (09:01)
    assert_eq!(rows[0]["key"], "site:chatgpt");
    assert_eq!(
        rows[0]["line"],
        "I'll ask ChatGPT for a copy of your data at 09:01"
    );
    assert_eq!(
        rows[1]["line"],
        "I'll ask Claude for a copy of your data at 09:01"
    );
    assert_eq!(
        status(&d).await["live"],
        card,
        "the Memory page reads the same card"
    );
    // ChatGPT's first run went well: it asked then, and now waits for the mail
    let chatgpt = made
        .iter()
        .find(|s| s.targets["job"] == "memory_import:chatgpt")
        .unwrap()
        .id
        .clone();
    d.on(ended(chatgpt.clone(), "done")).await;
    let card = cards(&d).last().cloned().flatten().unwrap();
    assert_eq!(card["rows"][0]["stage"], "waiting_mail");
    assert_eq!(
        card["rows"][0]["line"],
        "Waiting for ChatGPT's email — asked at 09:00"
    );
    assert_eq!(card["rows"][0]["note"], "");
    // a later check that needed the owner: said with when the next try is
    d.on(ended(chatgpt, "failed")).await;
    let card = cards(&d).last().cloned().flatten().unwrap();
    assert_eq!(
        card["rows"][0]["line"],
        "Waiting for ChatGPT's email — asked at 09:00"
    );
    assert!(
        card["rows"][0]["note"]
            .as_str()
            .unwrap()
            .starts_with("The last try didn't go through. I'll try again "),
        "{card}"
    );
}
