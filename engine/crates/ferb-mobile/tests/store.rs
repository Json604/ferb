//! The SQLCipher store (PHN-448): encrypted under the app's key, refused under another, its own schema upgraded from
//! v1, a newer one refused, entries kept by newest revision.

mod common;

use common::longest;
use ferb_mobile::store::{Cursor, Older, Store, StoreError, MIGRATIONS, VERSION};
use rusqlite::Connection;

const KEY: [u8; 32] = [3u8; 32];

fn raw(path: &std::path::Path, key: &[u8; 32]) -> Connection {
    let c = Connection::open(path).unwrap();
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    c.execute_batch(&format!("PRAGMA key = \"x'{hex}'\";"))
        .unwrap();
    c
}

// decision: PHN-448
// decision: PHN-474
#[test]
fn the_file_is_encrypted_and_opens_only_with_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.db");
    let (_, entries) = longest();
    {
        let mut s = Store::open(&path, KEY.to_vec()).unwrap();
        assert_eq!(s.version().unwrap(), VERSION);
        let kept = s.put_entries("C1", &entries, "now").unwrap();
        assert_eq!(kept.len(), entries.len());
        assert!(
            s.put_entries("C1", &entries, "now").unwrap().is_empty(),
            "the same copies again keep nothing"
        );
        s.set_cursor(
            "C1",
            &Cursor {
                epoch: "E1".into(),
                after_seq: 9,
            },
        )
        .unwrap();
        s.set_cursor(
            "C1",
            &Cursor {
                epoch: "E1".into(),
                after_seq: 4,
            },
        )
        .unwrap();
        assert_eq!(
            s.cursor("C1").unwrap().unwrap().after_seq,
            9,
            "a cursor only moves forward"
        );
        s.set_older("C1", &Older::Before(12)).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        !bytes.starts_with(b"SQLite format 3"),
        "no plain SQLite header"
    );
    let words = entries[0].body["text"].as_str().unwrap_or("hi there");
    assert!(
        !bytes.windows(words.len()).any(|w| w == words.as_bytes()),
        "no stored words in the clear"
    );
    assert_eq!(
        Store::open(&path, [4u8; 32].to_vec()).err(),
        Some(StoreError::WrongKey)
    );
    assert_eq!(
        Store::open(&path, vec![1, 2, 3]).err(),
        Some(StoreError::BadKey)
    );
    let s = Store::open(&path, KEY.to_vec()).unwrap();
    assert_eq!(s.entries("C1").unwrap().len(), entries.len());
    assert_eq!(s.older("C1").unwrap(), Older::Before(12));
}

/// A store written at schema v1 opens upgraded, its rows kept.
// decision: PHN-448
// decision: PHN-474
#[test]
fn a_version_1_store_is_upgraded_with_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v1.db");
    {
        let c = raw(&path, &KEY);
        c.execute_batch(MIGRATIONS[0]).unwrap();
        c.execute_batch("PRAGMA user_version = 1").unwrap();
        c.execute(
            "INSERT INTO conversations (id, epoch, after_seq, older, updated_at) VALUES ('C1', 'E1', 7, 'complete', 'then')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO entries VALUES ('C1', 'x1', 4294967296, 7, 1, 0, '{\"id\":\"x1\",\"kind\":\"notice\",\"pos\":4294967296,\"seq\":7,\"rev\":1,\"body\":{\"text\":\"kept\"}}')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO outbox (id, n, text, created_at, state) VALUES ('sub-1', 1, 'waiting words', 'then', 'waiting')",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path, KEY.to_vec()).unwrap();
    assert_eq!(s.version().unwrap(), VERSION);
    assert_eq!(
        s.cursor("C1").unwrap(),
        Some(Cursor {
            epoch: "E1".into(),
            after_seq: 7
        })
    );
    assert_eq!(s.older("C1").unwrap(), Older::Complete);
    assert_eq!(s.entries("C1").unwrap()[0].body["text"], "kept");
    assert_eq!(s.outbox().unwrap()[0].text, "waiting words");
    // v3: a message's attachments, none for the older one
    assert!(s.outbox().unwrap()[0].attachments.is_empty());
    s.add_pending_get("C1", "x1").unwrap();
    assert_eq!(s.pending_gets("C1").unwrap(), ["x1"]);
    s.set_read_mark("C1", 5).unwrap();
    assert_eq!(s.read_mark("C1").unwrap(), Some(5));
}

/// A file from a newer app is refused, never misread.
// decision: PHN-448
// decision: PHN-474
#[test]
fn a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.db");
    drop(Store::open(&path, KEY.to_vec()).unwrap());
    raw(&path, &KEY)
        .execute_batch("PRAGMA user_version = 99")
        .unwrap();
    assert_eq!(
        Store::open(&path, KEY.to_vec()).err(),
        Some(StoreError::Newer(99))
    );
}

/// Dropping a conversation (reset) empties its entries and cursor; forgetting deletes the file.
// decision: PHN-408
// decision: PHN-474
#[test]
fn a_reset_drops_what_was_fetched_and_forget_deletes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.db");
    let (_, entries) = longest();
    let mut s = Store::open(&path, KEY.to_vec()).unwrap();
    s.put_entries("C1", &entries, "now").unwrap();
    s.set_cursor(
        "C1",
        &Cursor {
            epoch: "E1".into(),
            after_seq: 3,
        },
    )
    .unwrap();
    s.drop_conversation("C1").unwrap();
    assert!(s.entries("C1").unwrap().is_empty());
    assert_eq!(s.cursor("C1").unwrap(), None);
    s.forget().unwrap();
    assert!(!path.exists());
}
