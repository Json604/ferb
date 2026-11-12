//! The store's guarantees, each as `tests/test_migrations.py`, `tests/test_upgrade.py` and
//! `tests/test_storage_fixes.py` state them for Python, plus the comparison with the store the
//! Python code makes (`rust/fixtures/store/make.py`, decision: ENG-3). Temporary folders only.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use ferb_store::{known_schema, split_sql, Db, Error, Migration};
use rusqlite::types::Value as Sql;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const PRAGMAS: [&str; 10] = [
    "auto_vacuum",
    "journal_mode",
    "journal_size_limit",
    "foreign_keys",
    "secure_delete",
    "busy_timeout",
    "page_size",
    "encoding",
    "user_version",
    "application_id",
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/store")
}

fn write(folder: &Path, name: &str, sql: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(folder.join(name), sql).unwrap();
}

fn table_exists(db: &Db, name: &str) -> bool {
    db.conn()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = ?",
            [name],
            |r| r.get(0),
        )
        .unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// What `make.py`'s `describe` reads from a store, read the same way.
fn describe(db: &Db) -> Value {
    let conn = db.conn();
    let rows = |sql: &str, columns: usize| -> Vec<Vec<Value>> {
        let mut statement = conn.prepare(sql).unwrap();
        let cell = |r: &rusqlite::Row, i: usize| {
            Ok(match r.get(i)? {
                Sql::Null => Value::Null,
                Sql::Integer(n) => json!(n),
                Sql::Text(t) => json!(t),
                other => panic!("unexpected value {other:?}"),
            })
        };
        let read = |r: &rusqlite::Row| (0..columns).map(|i| cell(r, i)).collect();
        statement
            .query_map([], read)
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let master = rows(
        "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name",
        4,
    );
    let master: Vec<Value> = master
        .into_iter()
        .map(|r| json!({"type": r[0], "name": r[1], "tbl_name": r[2], "sql": r[3]}))
        .collect();
    let pragma = |p: &&str| (p.to_string(), rows(&format!("PRAGMA {p}"), 1)[0][0].clone());
    json!({
        "schema_version": rows("SELECT version, sha256 FROM schema_version ORDER BY version", 2),
        "pragmas": PRAGMAS.iter().map(pragma).collect::<serde_json::Map<_, _>>(),
        "mode": format!("0o{:o}", mode(db.path())),
        "sqlite_master": master,
    })
}

// decisions: ENG-3, ENG-4, STORE-409, STORE-417
#[test]
fn a_fresh_store_is_the_one_python_makes() {
    let python: Value =
        serde_json::from_slice(&fs::read(fixtures().join("fresh_store.json")).unwrap()).unwrap();
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("deep/er/ferb.db")).unwrap();
    let applied = db.migrate().unwrap();
    let rust = describe(&db);

    assert_eq!(json!(applied), python["applied"]);
    assert_eq!(json!(known_schema()), python["known_schema"]);
    assert_eq!(
        rust["schema_version"], python["schema_version"],
        "the recorded versions and checksums"
    );
    assert_eq!(rust["pragmas"], python["pragmas"]);
    assert_eq!(rust["mode"], python["mode"]);
    let (ours, theirs) = (
        rust["sqlite_master"].as_array().unwrap(),
        python["sqlite_master"].as_array().unwrap(),
    );
    for (a, b) in ours.iter().zip(theirs) {
        assert_eq!(a, b);
    }
    assert_eq!(ours.len(), theirs.len());
    assert!(ours.len() > 200, "the fixture holds the whole schema");
}

// decision: STORE-414
#[test]
fn reopening_a_migrated_store_changes_nothing() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("ferb.db");
    let db = Db::open(&path).unwrap();
    assert_eq!(db.migrate().unwrap().len(), Migration::embedded().len());
    let before = describe(&db);
    assert_eq!(db.migrate().unwrap(), Vec::<i64>::new());
    db.close().unwrap();

    let db = Db::open(&path).unwrap();
    assert_eq!(db.migrate().unwrap(), Vec::<i64>::new());
    assert_eq!(db.schema_version().unwrap(), known_schema());
    assert_eq!(describe(&db), before);
}

// decision: STORE-409
#[test]
fn the_file_is_private_and_only_a_new_store_gets_incremental_auto_vacuum() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("ferb.db");
    let db = Db::open(&path).unwrap();
    assert_eq!(mode(&path), 0o600);
    assert!(db.incremental().unwrap());
    db.close().unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    Db::open(&path).unwrap();
    assert_eq!(mode(&path), 0o600, "set again at every open");

    // a store that already held a table when FERB first opened it keeps its setting
    let old = tmp.path().join("old.db");
    rusqlite::Connection::open(&old)
        .unwrap()
        .execute_batch("CREATE TABLE t (x)")
        .unwrap();
    assert!(!Db::open(&old).unwrap().incremental().unwrap());
}

// decision: STORE-414
#[test]
fn applies_in_order_and_records_each_file() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    write(&m, "0002_b.sql", "CREATE TABLE b (x INTEGER);");
    write(&m, "notes.sql", "NOT A MIGRATION");
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    assert_eq!(db.migrate_dir(&m).unwrap(), vec![1, 2]);
    assert_eq!(db.migrate_dir(&m).unwrap(), Vec::<i64>::new());
    assert_eq!(db.schema_version().unwrap(), 2);
}

// decisions: STORE-414, STORE-3
#[test]
fn a_database_newer_than_the_code_is_refused_and_left_alone() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    write(&m, "0002_b.sql", "CREATE TABLE b (x INTEGER);");
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate_dir(&m).unwrap();
    fs::remove_file(m.join("0002_b.sql")).unwrap();
    let before = describe(&db);
    let error = db.migrate_dir(&m).unwrap_err();
    assert!(
        matches!(
            error,
            Error::NewerStore {
                newest: 2,
                known: 1
            }
        ),
        "{error:?}"
    );
    assert!(error.is_migration_error());
    assert_eq!(
        error.to_string(),
        "FERB's database was last used by a newer FERB (schema 2; this FERB knows up to 1). Install the newer FERB; \
         nothing was changed"
    );
    assert_eq!(describe(&db), before);

    // as tests/test_upgrade.py:166: the real migrations against a store one version ahead
    let db = Db::open(tmp.path().join("real.db")).unwrap();
    db.migrate().unwrap();
    db.conn()
        .execute(
            "INSERT INTO schema_version (version) VALUES (?)",
            [known_schema() + 1],
        )
        .unwrap();
    assert!(matches!(
        db.migrate().unwrap_err(),
        Error::NewerStore { .. }
    ));
}

// decision: STORE-414
#[test]
fn an_applied_file_edited_afterwards_is_refused() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate_dir(&m).unwrap();
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER, y TEXT);");
    write(&m, "0002_b.sql", "CREATE TABLE b (x INTEGER);");
    let error = db.migrate_dir(&m).unwrap_err();
    assert_eq!(
        error.to_string(),
        "migration 0001_a.sql changed after it was applied; add a new migration instead"
    );
    assert!(
        !table_exists(&db, "b"),
        "nothing is applied once one file is refused"
    );
}

// decision: STORE-414
#[test]
fn a_new_file_numbered_below_the_newest_applied_is_refused_not_skipped() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    write(&m, "0003_c.sql", "CREATE TABLE c (x INTEGER);");
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.migrate_dir(&m).unwrap();
    write(&m, "0002_b.sql", "CREATE TABLE b (x INTEGER);");
    let error = db.migrate_dir(&m).unwrap_err();
    assert_eq!(
        error.to_string(),
        "migration 0002_b.sql is numbered below the newest applied (0003) and would never run on existing installs; \
         renumber it above 0003"
    );
    assert!(!table_exists(&db, "b"));
}

// decision: STORE-414
#[test]
fn two_files_with_one_number_are_refused() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    write(&m, "0001_b.sql", "CREATE TABLE b (x INTEGER);");
    let error = Db::open(tmp.path().join("ferb.db"))
        .unwrap()
        .migrate_dir(&m)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "two migrations are numbered 0001: 0001_a.sql, 0001_b.sql"
    );
}

// decisions: STORE-414, STORE-403
#[test]
fn rows_from_the_old_runner_are_accepted_and_get_their_checksum() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    write(&m, "0003_c.sql", "CREATE TABLE c (x INTEGER);");
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.conn()
        .execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             CREATE TABLE a (x INTEGER);
             CREATE TABLE c (x INTEGER);
             INSERT INTO schema_version (version) VALUES (1), (2), (3);",
        )
        .unwrap();
    assert_eq!(db.migrate_dir(&m).unwrap(), Vec::<i64>::new());
    let sums: Vec<Option<String>> = {
        let mut rows = db
            .conn()
            .prepare("SELECT sha256 FROM schema_version ORDER BY version")
            .unwrap();
        rows.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    // sha256 of the exact bytes of 0001_a.sql; the retired 0002 has no file and stays NULL
    let sum = |sql: &str| Some(hex::encode(Sha256::digest(sql)));
    assert_eq!(sums[0], sum("CREATE TABLE a (x INTEGER);"));
    assert_eq!(sums[1], None);
    assert_eq!(sums[2], sum("CREATE TABLE c (x INTEGER);"));
    write(&m, "0003_c.sql", "CREATE TABLE c (x INTEGER, y TEXT);");
    let error = db.migrate_dir(&m).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("migration 0003_c.sql changed"),
        "{error}"
    );
}

// decision: STORE-414
#[test]
fn a_failing_migration_leaves_nothing_behind() {
    let tmp = TempDir::new().unwrap();
    let m = tmp.path().join("m");
    write(&m, "0001_a.sql", "CREATE TABLE a (x INTEGER);");
    write(
        &m,
        "0002_b.sql",
        "CREATE TABLE b (x INTEGER);\nINSERT INTO b VALUES (1);\nTHIS IS NOT SQL;",
    );
    write(&m, "0003_c.sql", "CREATE TABLE c (x INTEGER);");
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    assert!(matches!(db.migrate_dir(&m).unwrap_err(), Error::Sqlite(_)));
    assert!(db.conn().is_autocommit(), "no transaction is left open");
    assert_eq!(
        db.schema_version().unwrap(),
        1,
        "the files before it stay applied"
    );
    assert!(table_exists(&db, "a") && !table_exists(&db, "b") && !table_exists(&db, "c"));

    // a statement cut off at the end of a file is refused the same way (db.py:275)
    write(
        &m,
        "0002_b.sql",
        "CREATE TABLE b (x INTEGER);\nCREATE TABLE cut (x INTEGER)",
    );
    let error = db.migrate_dir(&m).unwrap_err();
    assert_eq!(
        error.to_string(),
        "incomplete SQL statement at end of migration: CREATE TABLE cut (x INTEGER)"
    );
    assert!(!table_exists(&db, "b"));
}

#[test]
fn a_migration_splits_into_statements_as_python_splits_it() {
    let script = "-- a comment first\n\nCREATE TABLE a (\n  x INTEGER  -- kept; inside\n);\n-- between\n\
                  CREATE TRIGGER t AFTER INSERT ON a BEGIN\n  DELETE FROM a WHERE x = 1;\n  DELETE FROM a;\nEND;\r\n";
    assert_eq!(
        split_sql(script).unwrap(),
        vec![
            "CREATE TABLE a (\n  x INTEGER  -- kept; inside\n);",
            "CREATE TRIGGER t AFTER INSERT ON a BEGIN\n  DELETE FROM a WHERE x = 1;\n  DELETE FROM a;\nEND;",
        ]
    );
}

// decision: STORE-415
#[test]
fn a_transaction_commits_whole_or_not_at_all() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    db.conn()
        .execute_batch("CREATE TABLE t (x INTEGER UNIQUE)")
        .unwrap();
    let count = |db: &Db| {
        db.conn()
            .query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };

    db.transaction(|c| c.execute("INSERT INTO t VALUES (1)", []))
        .unwrap();
    assert_eq!(count(&db), 1);

    let failed: Result<(), rusqlite::Error> = db.transaction(|c| {
        c.execute("INSERT INTO t VALUES (2)", [])?;
        c.execute("INSERT INTO t VALUES (1)", [])?;
        Ok(())
    });
    assert!(failed.is_err());
    assert!(db.conn().is_autocommit());
    assert_eq!(
        count(&db),
        1,
        "the first insert of the failed block is gone"
    );

    // inside a transaction already, the block is part of that one: the outer error undoes it
    let outer: Result<(), Error> = db.transaction(|c| {
        db.transaction(|c| c.execute("INSERT INTO t VALUES (3)", []))?;
        assert!(!c.is_autocommit(), "the inner block did not commit");
        Err(Error::Migration("stop".into()))
    });
    assert!(outer.is_err());
    assert_eq!(count(&db), 1);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), Error> = db.transaction(|c| {
            c.execute("INSERT INTO t VALUES (4)", [])?;
            panic!("mid-transaction")
        });
    }));
    assert!(panicked.is_err());
    assert!(db.conn().is_autocommit());
    assert_eq!(count(&db), 1);
}

// decision: STORE-409
#[test]
fn the_wal_is_emptied_unless_a_reader_holds_it() {
    use std::sync::{Arc, Mutex};
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("ferb.db");
    let wal = tmp.path().join("ferb.db-wal");
    let mut db = Db::open(&path).unwrap();
    db.conn()
        .execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1)")
        .unwrap();
    assert!(fs::metadata(&wal).unwrap().len() > 0);
    assert_eq!(
        ferb_store::store_size(&path),
        fs::metadata(&path).unwrap().len() + fs::metadata(&wal).unwrap().len()
    );
    assert!(db.checkpoint().unwrap());
    assert_eq!(fs::metadata(&wal).unwrap().len(), 0);
    assert_eq!(
        db.conn()
            .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5000
    );

    // a reader in the middle of a read keeps the WAL: the routine checkpoint returns at once
    let reader = Db::open(&path).unwrap();
    reader
        .conn()
        .execute_batch("BEGIN; SELECT * FROM t")
        .unwrap();
    db.conn().execute_batch("INSERT INTO t VALUES (2)").unwrap();
    let started = std::time::Instant::now();
    assert!(!db.checkpoint().unwrap());
    assert!(started.elapsed().as_secs() < 2, "no busy wait");
    assert_eq!(
        db.conn()
            .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5000
    );

    // the forget's truncate tells what was left (its wait set to none here: a test never waits on real time, TEST-508)
    let told = Arc::new(Mutex::new(Vec::new()));
    let sink = told.clone();
    db.on_wal_kept = Box::new(move |after, kept| {
        sink.lock()
            .unwrap()
            .push((after.to_string(), kept.wal_frames))
    });
    db.conn().execute_batch("PRAGMA busy_timeout=0").unwrap();
    assert!(!db.truncate_wal("forget").unwrap());
    assert_eq!(told.lock().unwrap().len(), 1);
    assert_eq!(told.lock().unwrap()[0].0, "forget");
    reader.conn().execute_batch("COMMIT").unwrap();
    assert!(db.truncate_wal("forget").unwrap());
    assert_eq!(told.lock().unwrap().len(), 1);
    assert_eq!(fs::metadata(&wal).unwrap().len(), 0);
}

// decisions: STORE-409, STORE-416
#[test]
fn compaction_needs_room_and_makes_an_old_store_incremental() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("old.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (x)")
        .unwrap();
    let db = Db::open(&path).unwrap();
    db.conn()
        .execute_batch("INSERT INTO t VALUES (zeroblob(400000)); DELETE FROM t")
        .unwrap();
    let needed = db.compaction_needs().unwrap();
    assert!(needed > 0);

    let refused = db.compact(|_| Ok(needed - 1)).unwrap();
    assert!(!refused.compacted && refused.needed == needed && refused.free == needed - 1);
    assert!(!db.incremental().unwrap());

    let before = fs::metadata(&path).unwrap().len();
    assert!(db.compact(ferb_store::disk_free).unwrap().compacted);
    assert!(db.incremental().unwrap());
    assert!(fs::metadata(&path).unwrap().len() < before);

    db.conn()
        .execute_batch("INSERT INTO t VALUES (zeroblob(400000)); DELETE FROM t")
        .unwrap();
    assert!(db.vacuum_slice(8).unwrap() > 0);
    while db.vacuum_slice(ferb_store::VACUUM_SLICE_PAGES).unwrap() > 0 {}
}

/// Each daemon opens the other's store (decisions: ENG-3, ENG-4). Runs the Python code, so it is
/// not part of the default run: `cargo test -p ferb-store -- --ignored` (needs `uv`).
#[test]
#[ignore = "runs the Python store code through uv"]
fn python_and_rust_open_each_others_store() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let python = |command: &str, store: &Path| {
        let out = Command::new("uv")
            .args(["run", "python", "rust/fixtures/store/make.py", command])
            .arg(store)
            .current_dir(&repo)
            .output()
            .expect("uv is needed for this test");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    let tmp = TempDir::new().unwrap();

    // made by Rust, opened by Python: nothing left to apply, nothing refused, the same records
    let ours = tmp.path().join("rust.db");
    let db = Db::open(&ours).unwrap();
    db.migrate().unwrap();
    let mut rust = describe(&db);
    db.close().unwrap();
    let seen: Value = serde_json::from_slice(&python("accept", &ours)).unwrap();
    println!(
        "python accept (store made by Rust): applied={} mode={}",
        seen["applied"], seen["mode"]
    );
    assert_eq!(seen["applied"], json!([]));
    rust.as_object_mut().unwrap().remove("sqlite_master");
    for key in ["schema_version", "pragmas", "mode"] {
        assert_eq!(seen[key], rust[key], "{key}");
    }

    // made by Python, opened by Rust
    let theirs = tmp.path().join("python.db");
    python("create", &theirs);
    let db = Db::open(&theirs).unwrap();
    assert_eq!(db.migrate().unwrap(), Vec::<i64>::new());
    assert_eq!(db.schema_version().unwrap(), known_schema());
    println!(
        "rust migrate (store made by Python): applied=[] schema={}",
        db.schema_version().unwrap()
    );
}
