//! The image redaction's journal rows (HAND-531: `image.redaction_failed`, `image.redaction_recovered`) are
//! operational records, pruned as the previews' `preview.failed` notes are (owner, HAND-547): 90 days on, unless
//! their task is still live. Invented rows in a temporary store.

use ferb_store::{retention, Db};
use rusqlite::params;
use tempfile::TempDir;

fn store() -> (TempDir, Db) {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    (tmp, db)
}

fn row(db: &Db, id: &str, at: &str, kind: &str, data: &str) -> i64 {
    db.conn()
        .execute(
            "INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
            params![id, at, kind, data],
        )
        .unwrap();
    db.conn().last_insert_rowid()
}

// decision: HAND-547
#[test]
fn image_redaction_rows_go_when_the_previews_notes_go() {
    let (_tmp, db) = store();
    let old = "2026-01-01T09:00:00+00:00";
    let new = "2026-06-01T09:00:00+00:00";
    let faults = r#"{"use": "tool", "faults": [{"step": "vision", "domain": "com.apple.Vision", "code": 9}]}"#;
    let failed = row(&db, "j1", old, "image.redaction_failed", faults);
    let recovered = row(
        &db,
        "j2",
        old,
        "image.redaction_recovered",
        r#"{"use": "preview", "read_by": "vision_cpu", "faults": []}"#,
    );
    let preview = row(&db, "j3", old, "preview.failed", r#"{"error": "x"}"#);
    row(
        &db,
        "j4",
        old,
        "chat.notice",
        r#"{"id": "n1", "text": "kept"}"#,
    );
    row(&db, "j5", new, "image.redaction_failed", faults);
    let found = retention::operational(db.conn(), "2026-04-01T00:00:00+00:00").unwrap();
    let journal = found
        .iter()
        .find(|(t, _)| *t == "journal")
        .map(|(_, ids)| ids.clone())
        .unwrap_or_default();
    assert_eq!(journal, vec![failed, recovered, preview]);
    let mut sorted = retention::OPERATIONAL_KINDS.to_vec();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        retention::OPERATIONAL_KINDS.to_vec(),
        "bound in sorted order"
    );
}
