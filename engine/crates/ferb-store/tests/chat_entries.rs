//! Stage 2's conversation store (migration 0063, `chat_entries`): the carried history, the counters, pages and
//! changes, revisions and tombstones, cursors, submissions, marks, a forgotten notice and the one-time archive import.
//! No Python counterpart (Python never had 0063). Temporary folders only.

use std::{fs, path::Path};

use ferb_core::chat_entry::{
    body, Entry, EntryKind, EntryState, LegacyBody, NoticeBody, NoticeQuote,
};
use ferb_store::{
    chat_entries::{self as ce, Accept, CursorCheck, Submission, POS_BASE},
    Db, Error, Migration,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tempfile::TempDir;

/// A store migrated to 0062: the schema the Python build left (STORE-421's archive, and every store before stage 2).
fn store_at_0062(path: &Path) -> Db {
    let db = Db::open(path).unwrap();
    let older: Vec<Migration> = Migration::embedded()
        .into_iter()
        .filter(|m| m.name.as_ref() < "0063")
        .collect();
    db.migrate_files(&older).unwrap();
    assert_eq!(db.schema_version().unwrap(), 62);
    db
}

/// One chat_history row as FERB.app wrote it (`chat_history.write`): `body` is the stored bytes.
fn history(conn: &Connection, id: &str, kind: &str, at: &str, reference: Option<&str>, body: &str) {
    conn.execute(
        "INSERT INTO chat_history (id, kind, at, day, open, ref, body_json, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?, ?)",
        params![id, kind, at, &at[..10], reference, body, format!("{at}~u")],
    )
    .unwrap();
}

fn dump(conn: &Connection, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = conn.prepare(sql).unwrap();
    let n = statement.column_count();
    statement
        .query_map([], |r| (0..n).map(|i| r.get(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn entry(id: &str, conversation: &str, kind: EntryKind, day: &str, body: Value) -> Entry {
    Entry {
        id: id.into(),
        conversation_id: conversation.into(),
        pos: 0,
        seq: 0,
        rev: 0,
        kind,
        state: EntryState::Done,
        open: false,
        request_id: None,
        task_id: None,
        approval_id: None,
        reference: None,
        answers: None,
        placement: None,
        origin: None,
        client_id: None,
        submission_id: None,
        journal_id: None,
        created_at: format!("{day}T09:00:00+00:00"),
        updated_at: format!("{day}T09:00:00+00:00"),
        day: day.into(),
        body,
        deleted: false,
    }
}

fn message(id: &str, day: &str, text: &str) -> Entry {
    let mut e = entry(id, "c1", EntryKind::Message, day, json!({"text": text}));
    e.request_id = Some(format!("r-{id}"));
    e.origin = Some("owner".into());
    e
}

fn fresh(tmp: &TempDir) -> Db {
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    db
}

fn imported_from(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT imported_from FROM chat_entries WHERE id = ?",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

fn stored_body(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT body_json FROM chat_entries WHERE id = ?",
        [id],
        |r| r.get(0),
    )
    .unwrap()
}

const TURN_B: &str = "{\"id\": \"B\", \"text\": \"caf\u{e9} \\\"quoted\\\"\", \"n\": 1.5, \"nested\": {\"z\": [1, 2]}}";

/// A store at 0062 with three chat_history rows (two at one time: id breaks the tie), journal rows, and the
/// current conversation `c1`.
fn upgraded(tmp: &TempDir) -> Db {
    let path = tmp.path().join("ferb.db");
    let db = store_at_0062(&path);
    let conn = db.conn();
    conn.execute_batch(
        "INSERT INTO conversations (id) VALUES ('c1'); \
         INSERT INTO current_conversation (singleton, conversation_id) VALUES (1, 'c1'); \
         INSERT INTO journal (id, at, kind, data_json) VALUES ('j1', '2026', 'chat.message', '{}'), \
         ('j2', '2026', 'chat.reply', '{}');",
    )
    .unwrap();
    history(
        conn,
        "C",
        "notice",
        "2026-01-02T08:00:00.000000+00:00",
        Some("n1"),
        "{\"text\": \"seen\", \"quotes\": [{\"text\": \"q\", \"origin\": \"web\"}]}",
    );
    history(
        conn,
        "B",
        "turn",
        "2026-01-01T08:00:00.000000+00:00",
        None,
        TURN_B,
    );
    history(
        conn,
        "A",
        "reply",
        "2026-01-01T08:00:00.000000+00:00",
        None,
        "{\"turnID\": \"B\"}",
    );
    db
}

// decisions: ENG-8, STORE-403, STORE-401, ENG-12
#[test]
fn a_store_at_0062_upgrades_with_its_history_carried_in_order_and_its_bytes_kept() {
    let tmp = TempDir::new().unwrap();
    let db = upgraded(&tmp);
    let before = dump(db.conn(), "SELECT * FROM chat_history ORDER BY rowid");
    // 0064 (ORCH-651), 0065 (T-1202, the paired phones), 0066 (APP-979), 0067 (MEM-436) and 0068 (PHN-828) follow 0063
    assert_eq!(
        db.migrate().unwrap(),
        vec![
            ce::SCHEMA_VERSION,
            ce::SCHEMA_VERSION + 1,
            ce::SCHEMA_VERSION + 2,
            ce::SCHEMA_VERSION + 3,
            ce::SCHEMA_VERSION + 4,
            ce::SCHEMA_VERSION + 5
        ]
    );
    let conn = db.conn();
    assert_eq!(
        dump(conn, "SELECT * FROM chat_history ORDER BY rowid"),
        before,
        "chat_history is untouched"
    );

    let page = ce::page(conn, "c1", None, None, 10).unwrap();
    assert!(!page.more);
    let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["h:A", "h:B", "h:C"], "at, then id");
    for (i, e) in page.entries.iter().enumerate() {
        assert_eq!(e.pos, POS_BASE - 3 + i as u64);
        assert_eq!(e.seq, i as u64 + 1);
        assert_eq!(
            (e.rev, e.kind, e.state, e.open, e.deleted),
            (1, EntryKind::Legacy, EntryState::Done, false, false)
        );
        assert_eq!(
            imported_from(conn, &e.id).as_deref(),
            Some(ce::FROM_CHAT_HISTORY)
        );
        assert!(
            e.request_id.is_none()
                && e.task_id.is_none()
                && e.placement.is_none()
                && e.origin.is_none()
        );
    }
    let b = &page.entries[1];
    assert_eq!(b.created_at, "2026-01-01T08:00:00.000000+00:00");
    assert_eq!(b.updated_at, "2026-01-01T08:00:00.000000+00:00~u");
    assert_eq!(b.day, "2026-01-01");
    // the app's bytes, verbatim inside the LegacyBody shape
    let stored = stored_body(conn, "h:B");
    assert_eq!(
        stored,
        format!("{{\"kind\": \"turn\", \"body\": {TURN_B}}}")
    );
    let legacy: LegacyBody = b.body_as().unwrap();
    assert_eq!(legacy.kind, "turn");
    assert_eq!(legacy.body, serde_json::from_str::<Value>(TURN_B).unwrap());
    assert_eq!(page.entries[2].reference.as_deref(), Some("n1"));

    let stream = ce::stream(conn, "c1").unwrap().unwrap();
    assert_eq!(
        (stream.last_seq, stream.last_pos, stream.low_pos),
        (3, POS_BASE - 1, POS_BASE - 3)
    );
    assert!(
        stream.epoch.len() == 32
            && stream
                .epoch
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(ce::journal_from(conn).unwrap(), Some(2));
    assert_eq!(ce::conversations(conn).unwrap(), ["c1"]);
    assert_eq!(ce::days(conn, "c1").unwrap(), ["2026-01-02", "2026-01-01"]);

    // the engine's first own entry follows the history at POS_BASE
    let made = ce::create(conn, message("m1", "2026-01-03", "hi")).unwrap();
    assert_eq!((made.pos, made.seq, made.rev), (POS_BASE, 4, 1));
}

// decisions: ENG-8, STORE-403, ENG-12
#[test]
fn without_a_current_conversation_the_history_goes_to_owner() {
    let tmp = TempDir::new().unwrap();
    let db = store_at_0062(&tmp.path().join("ferb.db"));
    history(
        db.conn(),
        "A",
        "turn",
        "2026-01-01T08:00:00.000000+00:00",
        None,
        "{}",
    );
    db.migrate().unwrap();
    let e = ce::get(db.conn(), "h:A").unwrap().unwrap();
    assert_eq!(
        (e.conversation_id.as_str(), e.pos, e.seq),
        ("owner", POS_BASE - 1, 1)
    );
    // an empty journal leaves no mark: every journal row is newer
    assert_eq!(ce::journal_from(db.conn()).unwrap(), None);
}

// decisions: ENG-8, ENG-12
#[test]
fn a_fresh_store_has_no_stream_and_its_first_entry_takes_pos_base() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    assert_eq!(ce::stream(conn, "c1").unwrap(), None);
    assert_eq!(ce::journal_from(conn).unwrap(), None);
    assert_eq!(ce::head(conn, "c1").unwrap(), 0);
    let s = ce::open_stream(conn, "c1", "e0").unwrap();
    assert_eq!(
        (s.epoch.as_str(), s.last_seq, s.last_pos, s.low_pos),
        ("e0", 0, POS_BASE - 1, POS_BASE)
    );
    assert_eq!(
        ce::open_stream(conn, "c1", "other").unwrap().epoch,
        "e0",
        "an existing stream keeps its epoch"
    );
    let made = ce::create(conn, message("m1", "2026-01-01", "hi")).unwrap();
    assert_eq!((made.pos, made.seq, made.rev), (POS_BASE, 1, 1));
    assert_eq!(ce::get(conn, "m1").unwrap().unwrap(), made);
    assert_eq!(imported_from(conn, "m1"), None);
    assert!(matches!(
        ce::create(conn, message("m1", "2026-01-01", "again")),
        Err(Error::Value(_))
    ));
    assert_eq!(ce::new_epoch().len(), 32);
    assert_ne!(ce::new_epoch(), ce::new_epoch());
}

// decisions: ENG-10, ENG-12
#[test]
fn pages_read_back_older_entries_oldest_first() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    for (i, day) in [
        "2026-01-01",
        "2026-01-01",
        "2026-01-01",
        "2026-01-02",
        "2026-01-02",
    ]
    .iter()
    .enumerate()
    {
        ce::create(conn, message(&format!("m{i}"), day, "x")).unwrap();
    }
    let ids = |p: &ce::Page| p.entries.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
    let newest = ce::page(conn, "c1", None, None, 2).unwrap();
    assert_eq!(
        (ids(&newest), newest.more),
        (vec!["m3".to_owned(), "m4".to_owned()], true)
    );
    let older = ce::page(conn, "c1", Some(newest.entries[0].pos), None, 2).unwrap();
    assert_eq!(
        (ids(&older), older.more),
        (vec!["m1".to_owned(), "m2".to_owned()], true)
    );
    let oldest = ce::page(conn, "c1", Some(older.entries[0].pos), None, 2).unwrap();
    assert_eq!((ids(&oldest), oldest.more), (vec!["m0".to_owned()], false));
    let day = ce::page(conn, "c1", None, Some("2026-01-01"), 2).unwrap();
    assert_eq!(
        (ids(&day), day.more),
        (vec!["m1".to_owned(), "m2".to_owned()], true)
    );
    let day = ce::page(conn, "c1", Some(POS_BASE + 1), Some("2026-01-01"), 5).unwrap();
    assert_eq!((ids(&day), day.more), (vec!["m0".to_owned()], false));
    assert!(ce::page(conn, "elsewhere", None, None, 5)
        .unwrap()
        .entries
        .is_empty());
    assert!(matches!(
        ce::page(conn, "c1", None, None, 0),
        Err(Error::Value(_))
    ));
    assert_eq!(ce::days(conn, "c1").unwrap(), ["2026-01-02", "2026-01-01"]);
    let recent: Vec<String> = ce::recent(conn, "c1", 2)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(recent, ["m3", "m4"]);
}

// decisions: ENG-8, ENG-10, STORE-401, ENG-12
#[test]
fn changes_give_each_entry_once_at_its_latest_state_and_a_tombstone_is_a_change() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    let one = ce::create(conn, message("m1", "2026-01-01", "a")).unwrap();
    ce::create(conn, message("m2", "2026-01-01", "b")).unwrap();
    ce::create(conn, message("m3", "2026-01-01", "c")).unwrap();

    let mut done = one.clone();
    done.state = EntryState::Done;
    done.body = json!({"text": "a!"});
    let r2 = ce::revise(conn, done.clone(), "t2").unwrap();
    assert_eq!(
        (r2.rev, r2.seq, r2.pos, r2.updated_at.as_str()),
        (2, 4, one.pos, "t2")
    );
    let same = ce::revise(conn, done.clone(), "t9").unwrap();
    assert_eq!(same, r2, "a revision that changes nothing is not made");
    assert_eq!(ce::head(conn, "c1").unwrap(), 4);
    done.open = true;
    let r3 = ce::revise(conn, done, "t3").unwrap();
    assert_eq!((r3.rev, r3.seq), (3, 5));
    assert!(matches!(
        ce::revise(conn, message("nope", "2026-01-01", "x"), "t"),
        Err(Error::Value(_))
    ));

    let gone = ce::tombstone(conn, "m2", "t4").unwrap().unwrap();
    assert_eq!(
        (gone.deleted, gone.rev, gone.seq, gone.open),
        (true, 2, 6, false)
    );
    assert_eq!(gone.body, json!({}));
    assert_eq!(stored_body(conn, "m2"), "{}");
    assert_eq!(
        ce::tombstone(conn, "m2", "t5").unwrap().unwrap(),
        gone,
        "a tombstone stays as it is"
    );
    assert_eq!(ce::tombstone(conn, "nope", "t5").unwrap(), None);
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM chat_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 3, "no row is ever deleted");

    let all = ce::changes(conn, "c1", 0, 10).unwrap();
    let seen: Vec<(&str, u64)> = all.entries.iter().map(|e| (e.id.as_str(), e.seq)).collect();
    assert_eq!(seen, [("m3", 3), ("m1", 5), ("m2", 6)]);
    assert_eq!((all.more, all.head), (false, 6));
    assert_eq!(all.entries[1], r3);
    let first = ce::changes(conn, "c1", 0, 2).unwrap();
    assert_eq!((first.entries.len(), first.more, first.head), (2, true, 6));
    let after = ce::changes(conn, "c1", 5, 10).unwrap();
    assert_eq!(after.entries, vec![gone.clone()]);
    assert!(ce::changes(conn, "c1", 6, 10).unwrap().entries.is_empty());
    // a page shows the tombstone too
    assert!(ce::page(conn, "c1", None, None, 10)
        .unwrap()
        .entries
        .iter()
        .any(|e| e.id == "m2" && e.deleted));
    // what a restart rebuilds from
    assert_eq!(ce::open_entries(conn, "c1").unwrap(), vec![r3]);
    assert_eq!(
        ce::of_requests(conn, "c1", &["r-m3", "r-m1"])
            .unwrap()
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["m1", "m3"]
    );
    assert_eq!(
        ce::conversation_of_request(conn, "r-m1")
            .unwrap()
            .as_deref(),
        Some("c1")
    );
    assert_eq!(ce::conversation_of_request(conn, "r-none").unwrap(), None);
}

// decisions: ENG-8, ENG-12
#[test]
fn put_stores_an_entry_only_as_the_counter_and_its_stored_revision_allow() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    let refused = |r: ferb_store::Result<()>| assert!(matches!(r, Err(Error::Value(_))), "{r:?}");
    let mut e = message("m1", "2026-01-01", "a");
    e.pos = POS_BASE;
    e.seq = 1;
    e.rev = 1;
    refused(ce::put(conn, &e)); // no stream
    ce::open_stream(conn, "c1", "e0").unwrap();
    refused(ce::put(conn, &e)); // seq 1 above the counter 0
    assert_eq!(ce::next_seq(conn, "c1").unwrap(), 1);
    e.task_id = Some("t1".into());
    ce::put(conn, &e).unwrap();
    assert_eq!(ce::get(conn, "m1").unwrap().unwrap(), e);
    assert_eq!(
        ce::conversation_of_task(conn, "t1").unwrap().as_deref(),
        Some("c1")
    );

    let seq = ce::next_seq(conn, "c1").unwrap();
    let mut later = e.clone();
    later.rev = 2;
    later.seq = 1;
    refused(ce::put(conn, &later)); // seq not above the stored one
    later.seq = seq;
    later.pos = POS_BASE + 7;
    refused(ce::put(conn, &later)); // pos is set once
    later.pos = POS_BASE;
    later.conversation_id = "c2".into();
    ce::open_stream(conn, "c2", "e2").unwrap();
    ce::next_seq(conn, "c2").unwrap();
    ce::next_seq(conn, "c2").unwrap();
    refused(ce::put(conn, &later)); // another conversation
    later.conversation_id = "c1".into();
    later.state = EntryState::Open;
    ce::put(conn, &later).unwrap();
    assert_eq!(ce::get(conn, "m1").unwrap().unwrap(), later);
    assert!(matches!(ce::next_seq(conn, "none"), Err(Error::Value(_))));

    // put covers the positions it is given, both ways
    let s0 = ce::next_seq(conn, "c1").unwrap();
    let mut low = message("m0", "2025-12-31", "older");
    (low.pos, low.seq, low.rev) = (POS_BASE - 5, s0, 1);
    ce::put(conn, &low).unwrap();
    let s = ce::stream(conn, "c1").unwrap().unwrap();
    assert_eq!(
        (s.low_pos, s.last_pos, s.last_seq),
        (POS_BASE - 5, POS_BASE, 3)
    );
}

// decisions: ENG-10, ENG-12
#[test]
fn a_cursor_from_another_epoch_resets_and_one_beyond_the_head_is_ahead() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    assert_eq!(
        ce::check_cursor(conn, "c1", "e0", 0).unwrap(),
        CursorCheck::ResetRequired { epoch: None }
    );
    ce::open_stream(conn, "c1", "e0").unwrap();
    ce::create(conn, message("m1", "2026-01-01", "a")).unwrap();
    assert_eq!(
        ce::check_cursor(conn, "c1", "e0", 0).unwrap(),
        CursorCheck::Ok { head: 1 }
    );
    assert_eq!(
        ce::check_cursor(conn, "c1", "e0", 1).unwrap(),
        CursorCheck::Ok { head: 1 }
    );
    assert_eq!(
        ce::check_cursor(conn, "c1", "e0", 2).unwrap(),
        CursorCheck::Ahead { head: 1 }
    );
    assert_eq!(
        ce::check_cursor(conn, "c1", "foreign", 1).unwrap(),
        CursorCheck::ResetRequired {
            epoch: Some("e0".into())
        }
    );
}

// decisions: ENG-10, ENG-12
#[test]
fn a_repeated_submission_returns_its_acceptance_and_other_words_conflict() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    let sum = ce::text_sha256("book [secret] a table");
    use sha2::Digest;
    assert_eq!(
        sum,
        hex::encode(sha2::Sha256::digest("book [secret] a table".as_bytes()))
    );
    assert_eq!(
        ce::text_sha256(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        ce::check_submission(conn, "phone", "s1", &sum).unwrap(),
        Accept::New
    );
    let accepted = Submission {
        client_id: "phone".into(),
        submission_id: "s1".into(),
        conversation_id: "c1".into(),
        request_id: "r1".into(),
        entry_id: "m1".into(),
        seq: 7,
        text_sha256: sum.clone(),
        accepted_at: "2026-01-01T09:00:00+00:00".into(),
    };
    db.transaction(|c| ce::record_submission(c, &accepted))
        .unwrap();
    assert_eq!(
        ce::check_submission(conn, "phone", "s1", &sum).unwrap(),
        Accept::Repeat(accepted.clone())
    );
    let other = ce::text_sha256("other words");
    assert_eq!(
        ce::check_submission(conn, "phone", "s1", &other).unwrap(),
        Accept::Conflict(accepted.clone())
    );
    assert_eq!(
        ce::check_submission(conn, "mac", "s1", &sum).unwrap(),
        Accept::New,
        "the key is per client"
    );
    assert!(
        ce::record_submission(conn, &accepted).is_err(),
        "a key is recorded once"
    );
}

// decisions: ENG-10, ENG-12
#[test]
fn marks_only_move_forward() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let conn = db.conn();
    ce::touch_client(conn, "phone", "t1").unwrap();
    ce::touch_client(conn, "phone", "t2").unwrap();
    let seen: (String, String) = conn
        .query_row(
            "SELECT first_seen, last_seen FROM chat_clients WHERE client_id = 'phone'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(seen, ("t1".into(), "t2".into()));
    assert_eq!(ce::marks(conn, "phone", "c1").unwrap(), None);
    let m = ce::mark(conn, "phone", "c1", Some(5), None, "t1").unwrap();
    assert_eq!(
        (m.delivered_seq, m.read_pos, m.updated_at.as_str()),
        (5, 0, "t1")
    );
    let m = ce::mark(conn, "phone", "c1", Some(3), Some(POS_BASE), "t2").unwrap();
    assert_eq!(
        (m.delivered_seq, m.read_pos, m.updated_at.as_str()),
        (5, POS_BASE, "t2")
    );
    let m = ce::mark(conn, "phone", "c1", Some(4), Some(POS_BASE - 1), "t3").unwrap();
    assert_eq!(
        (m.delivered_seq, m.read_pos, m.updated_at.as_str()),
        (5, POS_BASE, "t2"),
        "lower values are ignored"
    );
    let m = ce::mark(conn, "phone", "c1", None, None, "t4").unwrap();
    assert_eq!((m.delivered_seq, m.read_pos), (5, POS_BASE));
    assert_eq!(ce::marks(conn, "phone", "c1").unwrap(), Some(m));
    assert_eq!(ce::marks(conn, "mac", "c1").unwrap(), None);
}

// decisions: ENG-8, STORE-401, ENG-12
#[test]
fn a_cleared_notice_is_revised_in_the_entries_and_in_the_carried_history() {
    let tmp = TempDir::new().unwrap();
    let db = upgraded(&tmp);
    db.migrate().unwrap();
    let conn = db.conn();
    let mut notice = entry(
        "n-entry",
        "c1",
        EntryKind::Notice,
        "2026-01-03",
        body(&NoticeBody {
            text: "the words".into(),
            quotes: vec![NoticeQuote {
                text: "q".into(),
                origin: "web".into(),
            }],
            ..NoticeBody::default()
        }),
    );
    notice.reference = Some("n1".into());
    ce::create(conn, notice).unwrap();
    let head = ce::head(conn, "c1").unwrap();
    assert_eq!(
        db.transaction(|c| ce::clear_notice(c, "n1", "(forgotten)", "t9"))
            .unwrap(),
        2
    );
    let n = ce::get(conn, "n-entry").unwrap().unwrap();
    let said: NoticeBody = n.body_as().unwrap();
    assert_eq!(
        (
            said.text.as_str(),
            said.quotes.len(),
            n.rev,
            n.updated_at.as_str()
        ),
        ("(forgotten)", 0, 2, "t9")
    );
    let legacy = ce::get(conn, "h:C").unwrap().unwrap();
    let inner: LegacyBody = legacy.body_as().unwrap();
    assert_eq!(inner.body, json!({"text": "(forgotten)", "quotes": []}));
    assert_eq!((legacy.rev, legacy.updated_at.as_str()), (2, "t9"));
    let changed = ce::changes(conn, "c1", head, 10).unwrap();
    assert_eq!(changed.entries.len(), 2);
    assert_eq!(changed.head, head + 2);
    // saying it again changes nothing; other refs are untouched
    assert_eq!(
        ce::clear_notice(conn, "n1", "(forgotten)", "t10").unwrap(),
        0
    );
    assert_eq!(ce::clear_notice(conn, "other", "x", "t10").unwrap(), 0);
    assert_eq!(ce::head(conn, "c1").unwrap(), head + 2);
    assert_eq!(ce::refs(conn, "c1").unwrap(), ["n1"]);
}

/// A pre-Rust archive: a store at 0062 with its own chat_history, closed (so no write-ahead log is left).
fn archive(path: &Path, rows: &[(&str, &str, &str)]) {
    let db = store_at_0062(path);
    for (id, at, body) in rows {
        history(db.conn(), id, "turn", at, None, body);
    }
    db.close().unwrap();
}

fn listing(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

// decisions: ENG-10, STORE-421, STORE-401, ENG-12
#[test]
fn the_archive_is_imported_once_below_the_history_without_being_written() {
    let tmp = TempDir::new().unwrap();
    let db = upgraded(&tmp);
    db.migrate().unwrap();
    let conn = db.conn();
    ce::create(conn, message("m1", "2026-01-03", "now")).unwrap();
    let head = ce::head(conn, "c1").unwrap();
    assert_eq!(head, 4);

    let folder = tmp.path().join("archive dir é");
    fs::create_dir(&folder).unwrap();
    let path = folder.join("ferb.db");
    // B overlaps the carried history; the rest are older, out of id order
    archive(
        &path,
        &[
            (
                "Z",
                "2025-06-01T08:00:00.000000+00:00",
                "{\"text\": \"oldest\"}",
            ),
            ("B", "2026-01-01T08:00:00.000000+00:00", TURN_B),
            (
                "Y",
                "2025-07-01T08:00:00.000000+00:00",
                "{\"text\":  \"spaced\" }",
            ),
            (
                "X",
                "2025-07-01T08:00:00.000000+00:00",
                "{\"text\": \"tie\"}",
            ),
        ],
    );
    let files = listing(&folder);
    let bytes = fs::read(&path).unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();

    let got = ce::import_archive(&db, &path, "c1").unwrap();
    assert_eq!(
        got,
        ce::Imported {
            added: 3,
            skipped: 1
        }
    );
    let page = ce::page(conn, "c1", None, None, 100).unwrap();
    let order: Vec<(&str, u64, u64)> = page
        .entries
        .iter()
        .map(|e| (e.id.as_str(), e.pos, e.seq))
        .collect();
    assert_eq!(
        order,
        [
            ("h:Z", POS_BASE - 6, 5),
            ("h:X", POS_BASE - 5, 6),
            ("h:Y", POS_BASE - 4, 7),
            ("h:A", POS_BASE - 3, 1),
            ("h:B", POS_BASE - 2, 2),
            ("h:C", POS_BASE - 1, 3),
            ("m1", POS_BASE, 4),
        ],
        "chronological by pos, contiguous, new seqs above the head"
    );
    assert_eq!(
        imported_from(conn, "h:Z").as_deref(),
        Some(ce::FROM_ARCHIVE)
    );
    assert_eq!(
        imported_from(conn, "h:B").as_deref(),
        Some(ce::FROM_CHAT_HISTORY)
    );
    assert_eq!(
        stored_body(conn, "h:Y"),
        "{\"kind\": \"turn\", \"body\": {\"text\":  \"spaced\" }}"
    );
    let s = ce::stream(conn, "c1").unwrap().unwrap();
    assert_eq!(
        (s.low_pos, s.last_pos, s.last_seq),
        (POS_BASE - 6, POS_BASE, 7)
    );
    let changes = ce::changes(conn, "c1", head, 10).unwrap();
    assert_eq!(
        changes
            .entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["h:Z", "h:X", "h:Y"]
    );

    // once: a second run adds nothing
    assert_eq!(
        ce::import_archive(&db, &path, "c1").unwrap(),
        ce::Imported {
            added: 0,
            skipped: 4
        }
    );
    assert_eq!(ce::head(conn, "c1").unwrap(), 7);
    // the archive was never written, and nothing was made beside it
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert_eq!(listing(&folder), files);
}

// decisions: ENG-10, STORE-421, ENG-12
#[test]
fn an_archive_into_a_fresh_store_opens_its_stream_below_pos_base() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    let path = tmp.path().join("old.db");
    archive(
        &path,
        &[
            ("A", "2025-01-01T00:00:00.000000+00:00", "{}"),
            ("B", "2025-01-02T00:00:00.000000+00:00", "{}"),
        ],
    );
    assert_eq!(
        ce::import_archive(&db, &path, "c9").unwrap(),
        ce::Imported {
            added: 2,
            skipped: 0
        }
    );
    let s = ce::stream(db.conn(), "c9").unwrap().unwrap();
    assert_eq!(
        (s.low_pos, s.last_pos, s.last_seq),
        (POS_BASE - 2, POS_BASE - 1, 2)
    );
    // the engine's own entries still start at POS_BASE
    let mut m = message("m1", "2026-01-01", "x");
    m.conversation_id = "c9".into();
    assert_eq!(ce::create(db.conn(), m).unwrap().pos, POS_BASE);
}

// decisions: ENG-10, STORE-421, ENG-12
#[test]
fn a_missing_archive_is_an_error_and_one_without_chat_history_imports_nothing() {
    let tmp = TempDir::new().unwrap();
    let db = fresh(&tmp);
    assert!(matches!(
        ce::import_archive(&db, &tmp.path().join("missing.db"), "c1"),
        Err(Error::Io(_))
    ));
    assert!(!tmp.path().join("missing.db").exists());

    let bare = tmp.path().join("bare.db");
    {
        let c = Connection::open(&bare).unwrap();
        c.execute_batch("CREATE TABLE other (x INTEGER); INSERT INTO other VALUES (1);")
            .unwrap();
    }
    assert_eq!(
        ce::import_archive(&db, &bare, "c1").unwrap(),
        ce::Imported {
            added: 0,
            skipped: 0
        }
    );
    assert_eq!(ce::stream(db.conn(), "c1").unwrap(), None);

    // an immutable read would skip a write-ahead log: refused, nothing read or written
    let walled = tmp.path().join("walled.db");
    archive(&walled, &[("A", "2025-01-01T00:00:00.000000+00:00", "{}")]);
    fs::write(tmp.path().join("walled.db-wal"), b"not empty").unwrap();
    assert!(matches!(
        ce::import_archive(&db, &walled, "c1"),
        Err(Error::Value(_))
    ));
    assert_eq!(ce::head(db.conn(), "c1").unwrap(), 0);
}
