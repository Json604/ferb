//! Migration 0065 (stage 3 S3, T-1202): the paired phones and their audit. A store at 0064 with data upgrades with
//! every row kept (STORE-403); the live-phone limit is checked in the transaction that adds one; a removed phone's row
//! stays and its endpoint can be paired afresh as a new row; the audit keeps what it is given and prunes by age.

use ferb_store::phones::{self, AddRefused, AuditRow, NewPhone};
use ferb_store::{Db, Migration};
use serde_json::json;
use tempfile::TempDir;

fn new(endpoint: &str) -> NewPhone {
    NewPhone {
        endpoint: endpoint.into(),
        name: format!("Phone {endpoint}"),
        app: "FERB for Android".into(),
        unlock_pub: Some("lock-key".into()),
        device_pub: None,
        strong_pub: None,
    }
}

// decision: STORE-403
// decision: STORE-414
#[test]
fn a_store_at_0064_upgrades_with_its_rows_kept_and_no_phones() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    let upto: Vec<Migration> = Migration::embedded()
        .into_iter()
        .filter(|m| m.name.as_ref() < "0065")
        .collect();
    db.migrate_files(&upto).unwrap();
    db.conn()
        .execute_batch(
            "INSERT INTO conversations(id) VALUES ('owner'); \
             INSERT INTO journal (id, at, kind, data_json) VALUES ('J1', '2026-11-16T10:00:00+05:30', 'chat.message', '{\"text\": \"hi\"}');",
        )
        .unwrap();
    let journal = |db: &Db| -> Vec<(String, String)> {
        let mut st = db
            .conn()
            .prepare("SELECT id, data_json FROM journal ORDER BY seq")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let before = journal(&db);
    assert_eq!(
        db.migrate().unwrap(),
        vec![
            phones::SCHEMA_VERSION,
            ferb_store::chat_history::REPAIR_VERSION
        ]
    );
    assert_eq!(journal(&db), before);
    assert!(phones::live(db.conn()).unwrap().is_empty());
    let ok: String = db
        .conn()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
    // a second start applies nothing
    assert!(db.migrate().unwrap().is_empty());
}

// decision: PHN-419
// decision: PHN-469
#[test]
fn at_most_the_given_number_of_live_phones_and_one_row_per_live_endpoint() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    for n in 0..5 {
        let added = phones::add(&db, &new(&format!("e{n}")), "2026-11-16T10:00:00+05:30", 5)
            .unwrap()
            .unwrap();
        assert_eq!(added.endpoint, format!("e{n}"));
        assert_eq!(added.last_seen, added.paired_at);
    }
    assert_eq!(
        phones::add(&db, &new("e5"), "2026-11-16T10:00:00+05:30", 5).unwrap(),
        Err(AddRefused::Full { max: 5 })
    );
    // removing one makes room; the removed row stays; the same endpoint paired afresh is a new row
    let removed = phones::remove(db.conn(), "e0", "mac", "2026-11-16T11:00:00+05:30")
        .unwrap()
        .unwrap();
    assert!(
        phones::remove(db.conn(), "e0", "mac", "2026-11-16T11:00:00+05:30")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        phones::add(&db, &new("e1"), "2026-11-16T12:00:00+05:30", 5).unwrap(),
        Err(AddRefused::AlreadyPaired)
    );
    let again = phones::add(&db, &new("e0"), "2026-11-16T12:00:00+05:30", 5)
        .unwrap()
        .unwrap();
    assert_ne!(again.id, removed.id);
    let all = phones::all(db.conn()).unwrap();
    assert_eq!(all.len(), 6);
    assert_eq!(all[0].removed_by.as_deref(), Some("mac"));
    assert_eq!(phones::live(db.conn()).unwrap().len(), 5);
    // the unique index holds whatever the code does
    assert!(db
        .conn()
        .execute(
            "INSERT INTO phone_devices (endpoint, name, paired_at, last_seen) VALUES ('e1', 'x', 'a', 'a')",
            []
        )
        .is_err());
}

// decision: PHN-133
#[test]
fn keys_are_added_once_and_a_sighting_clears_the_warning() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let p = phones::add(&db, &new("e"), "t0", 5).unwrap().unwrap();
    let p2 = phones::add_keys(db.conn(), "e", Some("dev"), Some("strong"))
        .unwrap()
        .unwrap();
    assert_eq!(
        (p2.device_pub.as_deref(), p2.strong_pub.as_deref()),
        (Some("dev"), Some("strong"))
    );
    let p3 = phones::add_keys(db.conn(), "e", Some("other"), Some("other"))
        .unwrap()
        .unwrap();
    assert_eq!(p3.strong_pub.as_deref(), Some("strong"), "never replaced");
    phones::mark_warned(db.conn(), p.id, "t1").unwrap();
    assert_eq!(
        phones::by_endpoint(db.conn(), "e")
            .unwrap()
            .unwrap()
            .warned_at
            .as_deref(),
        Some("t1")
    );
    phones::seen(db.conn(), "e", "t2").unwrap();
    let p4 = phones::by_endpoint(db.conn(), "e").unwrap().unwrap();
    assert_eq!((p4.last_seen.as_str(), p4.warned_at), ("t2", None));
    assert!(phones::rename(db.conn(), "e", "Work phone").unwrap());
    assert!(!phones::rename(db.conn(), "nobody", "x").unwrap());
}

// decision: PHN-130
#[test]
fn the_audit_keeps_rows_in_order_and_prunes_by_age() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    for (at, method) in [
        ("2026-08-01T10:00:00+05:30", "settings.get"),
        ("2026-11-16T10:00:00+05:30", "settings.set"),
    ] {
        phones::audit(
            db.conn(),
            &AuditRow {
                at: at.into(),
                device: "e".into(),
                method: method.into(),
                class: "private".into(),
                outcome: "ok".into(),
                detail: json!({}),
            },
        )
        .unwrap();
    }
    assert_eq!(
        phones::prune_audit(db.conn(), "2026-08-18T10:00:00+05:30").unwrap(),
        1
    );
    let rows = phones::audit_tail(db.conn(), 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].method, "settings.set");
}
