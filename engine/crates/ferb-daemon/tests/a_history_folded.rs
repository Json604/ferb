//! MEM-461 on FERB.app's own saved chat: a notice the engine folded into the review is marked `folded` on the history
//! page as it goes out, so the app never draws it again, whether or not it was running when the fold was made (owner
//! 2026-11-23: "its blank after i approved all the memory messages"). Temporary folders only.

mod a_support;

use a_support::{Daemon, Raw, Seq};
use ferb_clock::clock::FakeClock;
use serde_json::{json, Value};

async fn call(d: &Daemon, method: &str, params: Value) -> Value {
    let mut raw = Raw::connect(d.server.path()).await;
    raw.send(r#"{"jsonrpc": "2.0", "id": 1, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    raw.recv().await;
    raw.send(&json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}).to_string())
        .await;
    serde_json::from_str(&raw.recv().await).unwrap()
}

fn saved(id: &str, at: &str, reference: &str, text: &str) -> Value {
    json!({"id": id, "kind": "notice", "at": at, "day": "2026-11-23", "open": false, "ref": reference,
           "body": {"id": reference, "entryID": id, "text": text, "quotes": [], "createdAt": 0}})
}

// decision: MEM-461
#[tokio::test]
async fn a_folded_notice_is_marked_on_the_apps_history_page() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    let put = call(&d, "chat.history.put", json!({"entries": [
        saved("00000000-0000-4000-8000-0000000000A1", "2026-11-22T18:30:00.000+00:00", "memory-always-on-m1", "Now always on: “The owner prefers tea.”"),
        saved("00000000-0000-4000-8000-0000000000A2", "2026-11-22T18:31:00.000+00:00", "memory-always-on-m2", "Now always on: “The owner uses Arc.”"),
        saved("00000000-0000-4000-8000-0000000000A3", "2026-11-22T18:32:00.000+00:00", "memory-import-r1", "I read what you've typed into Codex."),
    ]})).await;
    assert!(put.get("result").is_some(), "{put}");
    // the engine's own entries: one notice folded into the review, one not
    let entry = |id: &str, pos: i64, reference: &str, body: Value| {
        (
            "INSERT INTO chat_entries (id, conversation_id, pos, seq, rev, kind, state, open, ref, created_at, \
             updated_at, day, body_json) VALUES (?, 'owner', ?, ?, 2, 'notice', 'done', 0, ?, \
             '2026-11-23T00:00:00+05:30', '2026-11-23T03:40:12+05:30', '2026-11-23', ?)"
                .to_owned(),
            vec![json!(id), json!(pos), json!(pos), json!(reference), json!(body.to_string())],
        )
    };
    d.sql(vec![
        entry(
            "e1",
            1,
            "memory-always-on-m1",
            json!({"text": "Now always on", "folded": "memory-review"}),
        ),
        entry(
            "e2",
            2,
            "memory-always-on-m2",
            json!({"text": "Now always on"}),
        ),
    ])
    .await;
    let page = call(
        &d,
        "chat.history.page",
        json!({"day": "2026-11-23", "limit": 30}),
    )
    .await;
    let entries = page["result"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("{page}"));
    let folded: Vec<(Value, Value)> = entries
        .iter()
        .map(|e| {
            (
                e["id"].clone(),
                e.get("folded").cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    assert_eq!(
        folded,
        vec![
            (
                json!("00000000-0000-4000-8000-0000000000A1"),
                json!("memory-review")
            ),
            (json!("00000000-0000-4000-8000-0000000000A2"), Value::Null),
            (json!("00000000-0000-4000-8000-0000000000A3"), Value::Null)
        ]
    );
    // the entry as the app kept it is left as it is
    assert!(entries[0]["body"].get("folded").is_none());
}
