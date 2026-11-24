//! The owner's switches for the memory import and its window (MEM-458, MEM-459): the last month of every source; the
//! coding assistants' switch reads them and keeps up with them, off stops both; the websites' switch asks which
//! browser profile and makes the two watches, off cancels them; neither is a bare setting. Temporary folders only; no
//! model, browser or mail.

mod a_support;

use a_support::{Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::memory_import;
use serde_json::{json, Value};

async fn daemon(tmp: &std::path::Path) -> Daemon {
    // MEM-478: the import is paused in the shipped program; its own tests drive it as if resumed
    memory_import::unpause_for_tests();
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

/// Lines typed into Claude Code: `(session, ms since the epoch)`.
fn history(d: &Daemon, lines: &[(&str, u64)]) {
    let path = d.user.join(".claude/history.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines
        .iter()
        .map(|(s, t)| {
            format!("{{\"display\": \"I keep bees on my roof\", \"timestamp\": {t}, \"sessionId\": \"{s}\", \"project\": \"/Users/me/hive\"}}\n")
        })
        .collect();
    std::fs::write(path, text).unwrap();
}

async fn setting(d: &Daemon, key: &'static str) -> Option<Value> {
    d.on(move |st| Box::pin(async move { st.config_get("memory", key) }))
        .await
}

/// `settings.set`'s write refuses the switch's key.
async fn bare(d: &Daemon, key: &'static str) -> bool {
    d.on(move |st| {
        Box::pin(async move {
            ferb_daemon::areas_a::settings::write(&st.config, key, &json!(true)).is_err()
        })
    })
    .await
}

async fn runs(d: &Daemon) -> usize {
    d.on(|st| {
        Box::pin(async move {
            memory_import::job(&st).status().unwrap()["runs"]
                .as_array()
                .unwrap()
                .len()
        })
    })
    .await
}

async fn settle(d: &Daemon) {
    for _ in 0..400 {
        let running = d
            .on(|st| Box::pin(async move { memory_import::job(&st).running().unwrap().len() }))
            .await;
        if running == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

// decision: MEM-458
#[tokio::test]
async fn an_import_reads_only_the_last_month() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    // the clock is 2026-01-01 09:00 UTC: one session eleven days before, one 45 days before
    history(
        &d,
        &[("recent", 1_766_224_800_000), ("old", 1_763_200_800_000)],
    );
    let estimate = d
        .on(|st| {
            Box::pin(async move { memory_import::job(&st).estimate("claude_code", "").unwrap() })
        })
        .await;
    assert_eq!(estimate["conversations"], 1, "{estimate}");
    assert!(
        estimate["since"]
            .as_str()
            .unwrap()
            .starts_with("2025-12-01"),
        "{estimate}"
    );
}

// decision: MEM-459, MEM-466
#[tokio::test]
async fn the_coding_assistants_switch_reads_them_and_off_stops_reading_and_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    history(&d, &[("recent", 1_766_224_800_000)]);
    // never a bare setting: the switch starts and stops the import with the value
    assert!(
        bare(&d, "memory.local_import").await,
        "never a bare setting"
    );
    let on = call(
        &d,
        "memory.import.switch",
        json!({"which": "local", "on": true}),
    )
    .await;
    assert_eq!(
        on["result"]["said"],
        "Thanks! I'm reading through Claude Code now and I'll tell you what I learned."
    );
    settle(&d).await;
    assert_eq!(runs(&d).await, 1, "turning it on reads them now");
    assert_eq!(setting(&d, "local_import").await, Some(json!(true)));
    let off = call(
        &d,
        "memory.import.switch",
        json!({"which": "local", "on": false}),
    )
    .await;
    assert_eq!(
        off["result"]["said"],
        "Okay, I've stopped reading your coding assistants. What I already learned stays on the Memory page."
    );
    assert_eq!(setting(&d, "local_import").await, Some(json!(false)));
    // off: no update reads anything
    d.on(|st| {
        Box::pin(async move { memory_import::local_update(&st, memory_import::Tell::Never).await })
    })
    .await;
    assert_eq!(runs(&d).await, 1);
    let bad = call(
        &d,
        "memory.import.switch",
        json!({"which": "gemini", "on": true}),
    )
    .await;
    assert!(bad["error"].is_object(), "{bad}");
}

// decision: MEM-459, MEM-448, MEM-451
#[tokio::test]
async fn the_websites_switch_asks_the_browser_and_off_cancels_the_watches() {
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(tmp.path()).await;
    let label = "Chrome “Work”".to_owned();
    let profiles = vec![(label.clone(), "Chrome/Profile 1".to_owned())];
    // the browsers FERB sees (setup isn't done here, so nothing is asked by itself)
    d.on(move |st| Box::pin(async move { memory_import::offer(&st, profiles) }))
        .await;
    assert!(asks(&d).is_empty());
    let on = call(
        &d,
        "memory.import.switch",
        json!({"which": "sites", "on": true}),
    )
    .await;
    assert_eq!(
        on["result"]["said"],
        "Which browser are you signed in to ChatGPT and Claude on? I've asked in the chat."
    );
    let asked = asks(&d);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["kind"], "memory_sites");
    let ident = asked[0]["ident"].as_str().unwrap_or("").to_owned();
    let chosen = label.clone();
    d.on(move |st| {
        Box::pin(async move {
            let ident = if ident.is_empty() {
                json!({"options": {"Chrome “Work”": "Chrome/Profile 1"}}).to_string()
            } else {
                ident
            };
            memory_import::sites_answered(&st, "granted", &chosen, &ident);
        })
    })
    .await;
    let watching = d
        .on(|st| Box::pin(async move { st.schedules().all(&["active"]).unwrap().len() }))
        .await;
    assert_eq!(watching, 2, "one watch per site");
    assert_eq!(setting(&d, "sites_import").await, Some(json!(true)));
    // off: the watches are cancelled, nothing more is requested or fetched, the card goes
    let off = call(
        &d,
        "memory.import.switch",
        json!({"which": "sites", "on": false}),
    )
    .await;
    assert_eq!(
        off["result"]["said"],
        "Okay, I won't ask ChatGPT or Claude for your data. What I already learned stays on the Memory page."
    );
    let (active, card) = d
        .on(|st| {
            Box::pin(async move {
                (
                    st.schedules().all(&["active"]).unwrap().len(),
                    memory_import::refresh(&st),
                )
            })
        })
        .await;
    assert_eq!(active, 0);
    assert_eq!(card, None);
    assert_eq!(setting(&d, "sites_import").await, Some(json!(false)));
    assert!(
        bare(&d, "memory.sites_import").await,
        "never a bare setting"
    );
    // on again later: asked again which browser
    call(
        &d,
        "memory.import.switch",
        json!({"which": "sites", "on": true}),
    )
    .await;
    assert_eq!(asks(&d).len(), 2);
}
