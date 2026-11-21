//! FERB's alerts are never kept in the chat (owner APP-1105; APP-1106): the ones an earlier build saved as notice entries
//! (or that came in with FERB.app's imported chat) are marked `"alert": true` at the next start, a revision every client
//! receives and stops drawing; nothing is deleted, and every other notice is left as it was. After a recorded chat turn
//! (`plain_reply`) ran through the real supervisor on a temporary store.

mod common;
mod support_control;

use std::collections::BTreeSet;

use ferb_core::chat_entry::{Entry, EntryKind, EntryState};
use ferb_store::chat_entries as store;
use serde_json::{json, Value};

fn notice(id: &str, kind: EntryKind, reference: Option<&str>, body: Value) -> Entry {
    let at = "2026-11-21T10:21:29+05:30";
    Entry {
        id: id.into(),
        conversation_id: "owner".into(),
        pos: 0,
        seq: 0,
        rev: 1,
        kind,
        state: EntryState::Done,
        open: false,
        request_id: None,
        task_id: None,
        approval_id: None,
        reference: reference.map(str::to_string),
        answers: None,
        placement: None,
        origin: None,
        client_id: None,
        submission_id: None,
        journal_id: None,
        created_at: at.into(),
        updated_at: at.into(),
        day: "2026-11-21".into(),
        body,
        deleted: false,
    }
}

// decision: APP-1105, APP-1106
#[test]
fn alerts_saved_earlier_are_marked_at_the_start_and_nothing_else_changes() {
    let mut w = support_control::world("plain_reply");
    support_control::run(&mut w, "plain_reply");
    w.sup.chat_log().flush(w.sup.inner());
    {
        let db = w.db.lock().unwrap();
        let c = db.conn();
        for e in [
            notice(
                "n-hot",
                EntryKind::Notice,
                Some("insomnia-warning-w1"),
                json!({"text": "Your Mac is getting hot.", "quotes": [], "cards": null, "at": "x"}),
            ),
            notice(
                "n-db",
                EntryKind::Notice,
                Some("set-aside-ferb.db"),
                json!({"text": "FERB couldn't read its database.", "quotes": [], "cards": null, "at": "x"}),
            ),
            notice(
                "n-tea",
                EntryKind::Notice,
                Some("memory-always-on-1"),
                json!({"text": "Now always on: tea", "quotes": [], "cards": null, "at": "x"}),
            ),
            notice(
                "l-window",
                EntryKind::Legacy,
                None,
                json!({"kind": "notice", "body": {"id": "window-shown-t-site", "text": "Chrome came forward."}}),
            ),
            notice(
                "l-turn",
                EntryKind::Legacy,
                None,
                json!({"kind": "turn", "body": {"id": "window-shown-x"}}),
            ),
        ] {
            store::create(c, e).unwrap();
        }
    }
    let before = |id: &str| {
        store::get(w.db.lock().unwrap().conn(), id)
            .unwrap()
            .unwrap()
    };
    let tea = before("n-tea");
    let turn = before("l-turn");
    w.sup
        .chat_log()
        .after_restart(w.sup.inner(), &BTreeSet::new());
    let db = w.db.lock().unwrap();
    let get = |id: &str| store::get(db.conn(), id).unwrap().unwrap();
    for id in ["n-hot", "n-db", "l-window"] {
        let e = get(id);
        assert_eq!(e.body["alert"], json!(true), "{id}");
        assert_eq!(e.rev, 2, "{id}");
        assert!(!e.deleted, "{id}");
    }
    // the words are kept
    assert_eq!(get("n-hot").body["text"], json!("Your Mac is getting hot."));
    assert_eq!(
        get("l-window").body["body"]["text"],
        json!("Chrome came forward.")
    );
    assert_eq!(get("n-tea"), tea);
    assert_eq!(get("l-turn"), turn);
    drop(db);
    // a second start changes nothing more
    w.sup
        .chat_log()
        .after_restart(w.sup.inner(), &BTreeSet::new());
    let db = w.db.lock().unwrap();
    assert_eq!(store::get(db.conn(), "n-hot").unwrap().unwrap().rev, 2);
}
