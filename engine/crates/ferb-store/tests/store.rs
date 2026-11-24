//! The store's guarantees, each as `tests/test_migrations.py`, `tests/test_upgrade.py` and
//! `tests/test_storage_fixes.py` state them for Python, plus the comparison with the store the
//! Python code makes (`engine/fixtures/store/make.py`, decision: ENG-3). Temporary folders only.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use ferb_store::{chat_entries, known_schema, split_sql, Db, Error, Migration};
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

/// The schema objects migration 0063 adds over the store Python made (stage 2; the tables are
/// `chat_entries::TABLES`): exactly these, and nothing else differs.
// decisions: ENG-11, ENG-8
const STAGE_2_OBJECTS: [&str; 19] = [
    "chat_client_marks",
    "chat_clients",
    "chat_entries",
    "chat_entries_approval",
    "chat_entries_day",
    "chat_entries_from",
    "chat_entries_open",
    "chat_entries_ref",
    "chat_entries_request",
    "chat_entries_task",
    "chat_streams",
    "chat_submissions",
    "sqlite_autoindex_chat_client_marks_1",
    "sqlite_autoindex_chat_clients_1",
    "sqlite_autoindex_chat_entries_1",
    "sqlite_autoindex_chat_entries_2",
    "sqlite_autoindex_chat_entries_3",
    "sqlite_autoindex_chat_streams_1",
    "sqlite_autoindex_chat_submissions_1",
];

/// The schema objects migration 0065 adds (stage 3, T-1202; the tables are `phones::TABLES`): exactly these.
// decision: PHN-130
const STAGE_3_OBJECTS: [&str; 4] = [
    "phone_audit",
    "phone_audit_at",
    "phone_devices",
    "phone_devices_live",
];

/// The schema objects migration 0067 adds (the memory import's profile, MEM-436): exactly these.
// decision: MEM-436
const MEMORY_IMPORT_OBJECTS: [&str; 14] = [
    "memory_import_chats",
    "memory_import_exports",
    "memory_import_facts",
    "memory_import_facts_line",
    "memory_import_files",
    "memory_import_lines",
    "memory_import_lines_item",
    "memory_import_state",
    "sqlite_autoindex_memory_import_chats_1",
    "sqlite_autoindex_memory_import_exports_1",
    "sqlite_autoindex_memory_import_facts_1",
    "sqlite_autoindex_memory_import_files_1",
    "sqlite_autoindex_memory_import_lines_1",
    "sqlite_autoindex_memory_import_state_1",
];

/// ORCH-651's named difference (migration 0064): Python's `tasks` table as 0064 rebuilds it, its CHECK naming
/// done_with_gaps; any other object as recorded.
fn tasks_with_gaps(object: &Value) -> Value {
    if object["type"] != "table" || object["name"] != "tasks" {
        return object.clone();
    }
    let sql = object["sql"].as_str().unwrap();
    let from = "'done', 'done_unverified', 'failed',\n                                                 'cancelled', \
                'needs_user', 'interrupted', 'skipped', 'unknown'))";
    let to = "'done', 'done_unverified',\n                                                 'done_with_gaps', 'failed', \
              'cancelled', 'needs_user', 'interrupted',\n                                                 'skipped', \
              'unknown'))";
    assert!(sql.contains(from), "Python's tasks table: {sql}");
    let mut out = object.clone();
    out["sql"] = json!(sql.replace(from, to));
    out
}

// decisions: ENG-3, ENG-4, ENG-11, STORE-409, STORE-417, ORCH-651
// decision: PHN-130
// decision: PHN-828
#[test]
fn a_fresh_store_is_the_one_python_makes() {
    // Python's store, plus exactly the named stage-2 additions (migration 0063, ENG-11): the
    // fresh store less its 0063 version row and 0063 objects equals Python's, strictly.
    let python: Value =
        serde_json::from_slice(&fs::read(fixtures().join("fresh_store.json")).unwrap()).unwrap();
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("deep/er/ferb.db")).unwrap();
    let applied = db.migrate().unwrap();
    let rust = describe(&db);
    let stage_2 = chat_entries::SCHEMA_VERSION;
    // 0064 (ORCH-651) rebuilds `tasks` with done_with_gaps in its CHECK: the one other named addition
    let gaps = stage_2 + 1;
    // 0065 (T-1202): stage 3's paired phones and their audit
    let phones = ferb_store::phones::SCHEMA_VERSION;
    assert_eq!(phones, gaps + 1);
    // 0066 (APP-979): a data repair of chat_history, no schema object of its own
    let repair = ferb_store::chat_history::REPAIR_VERSION;
    assert_eq!(repair, phones + 1);
    // 0067 (MEM-436): the memory import's profile
    let memory = repair + 1;
    // 0068 (PHN-828): each paired phone's FERB account, a column of 0065's phone_devices
    let account = ferb_store::phones::ACCOUNT_VERSION;
    assert_eq!(account, memory + 1);

    assert_eq!(
        applied[applied.len() - 6..],
        [stage_2, gaps, phones, repair, memory, account]
    );
    assert_eq!(json!(applied[..applied.len() - 6]), python["applied"]);
    assert_eq!(known_schema(), account);
    assert_eq!(json!(stage_2 - 1), python["known_schema"]);
    let versions = rust["schema_version"].as_array().unwrap();
    let (before, last) = versions.split_at(versions.len() - 6);
    assert_eq!(last[0][0], json!(stage_2));
    assert_eq!(last[1][0], json!(gaps));
    assert_eq!(last[2][0], json!(phones));
    assert_eq!(last[3][0], json!(repair));
    assert_eq!(last[4][0], json!(memory));
    assert_eq!(last[5][0], json!(account));
    assert_eq!(
        json!(before),
        python["schema_version"],
        "the recorded versions and checksums"
    );
    assert_eq!(rust["pragmas"], python["pragmas"]);
    assert_eq!(rust["mode"], python["mode"]);
    let (memory_added, all): (Vec<&Value>, Vec<&Value>) = rust["sqlite_master"]
        .as_array()
        .unwrap()
        .iter()
        .partition(|o| MEMORY_IMPORT_OBJECTS.contains(&o["name"].as_str().unwrap()));
    let mut memory_names: Vec<&str> = memory_added
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    memory_names.sort_unstable();
    assert_eq!(
        memory_names, MEMORY_IMPORT_OBJECTS,
        "0067 adds exactly the named objects"
    );
    let (phones_added, rest): (Vec<&Value>, Vec<&Value>) = all
        .into_iter()
        .partition(|o| STAGE_3_OBJECTS.contains(&o["name"].as_str().unwrap()));
    let mut phone_names: Vec<&str> = phones_added
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    phone_names.sort_unstable();
    assert_eq!(
        phone_names, STAGE_3_OBJECTS,
        "0065 adds exactly the named objects"
    );
    assert!(phones_added
        .iter()
        .all(|o| ferb_store::phones::TABLES.contains(&o["tbl_name"].as_str().unwrap())));
    let (added, ours): (Vec<&Value>, Vec<&Value>) = rest
        .into_iter()
        .partition(|o| STAGE_2_OBJECTS.contains(&o["name"].as_str().unwrap()));
    let theirs = python["sqlite_master"].as_array().unwrap();
    for (a, b) in ours.iter().zip(theirs) {
        assert_eq!(*a, &tasks_with_gaps(b));
    }
    assert_eq!(ours.len(), theirs.len());
    assert!(ours.len() > 200, "the fixture holds the whole schema");
    let mut names: Vec<&str> = added.iter().map(|o| o["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names, STAGE_2_OBJECTS,
        "0063 adds exactly the named objects"
    );
    let mut tables: Vec<&str> = added
        .iter()
        .filter(|o| o["type"] == "table")
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    tables.sort_unstable();
    let mut named = chat_entries::TABLES.to_vec();
    named.sort_unstable();
    assert_eq!(tables, named);
    assert!(added
        .iter()
        .all(|o| chat_entries::TABLES.contains(&o["tbl_name"].as_str().unwrap())));
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

/// ORCH-651, migration 0064: a populated store at 0063 upgrades with every task row (rowid, every column) and every
/// task_context row kept, both indexes made again, the foreign keys intact; afterwards `tasks` takes done_with_gaps,
/// which the old CHECK refused, and still refuses a status outside the list.
// decision: ORCH-651
#[test]
fn a_populated_store_at_0063_upgrades_to_take_done_with_gaps() {
    let tmp = TempDir::new().unwrap();
    let db = Db::open(tmp.path().join("ferb.db")).unwrap();
    let all = Migration::embedded();
    let upto: Vec<Migration> = all
        .iter()
        .filter(|m| m.name.as_ref() < "0064")
        .cloned()
        .collect();
    db.migrate_files(&upto).unwrap();
    let c = db.conn();
    let statuses = [
        "planned",
        "running",
        "paused",
        "done",
        "done_unverified",
        "failed",
        "cancelled",
        "needs_user",
        "interrupted",
        "skipped",
        "unknown",
    ];
    for (n, status) in statuses.iter().enumerate() {
        c.execute(
            "INSERT INTO tasks (rowid, id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
             outcome_json, created_at, updated_at) VALUES (?, ?, 't0', NULL, 'r', 'a goal', 'owner', 'worker', '[]', ?, \
             '{\"summary\": \"s\"}', '2026-01-01', '2026-01-02')",
            rusqlite::params![100 + n as i64, format!("t{n}"), status],
        )
        .unwrap();
    }
    c.execute("INSERT INTO conversations(id) VALUES ('owner')", [])
        .unwrap();
    c.execute(
        "INSERT INTO task_context (rowid, task_id, conversation_id, spec_json, owner_words, prior_task, request_payload) \
         VALUES (7, 't1', 'owner', '{}', 'words', 't0', 'payload')",
        [],
    )
    .unwrap();
    let refused = c.execute(
        "UPDATE tasks SET status = 'done_with_gaps' WHERE id = 't0'",
        [],
    );
    assert!(refused.is_err(), "0049's CHECK refuses the new state");
    let dump = |c: &rusqlite::Connection, sql: &str| -> Vec<String> {
        let mut st = c.prepare(sql).unwrap();
        let n = st.column_count();
        st.query_map([], |r| {
            Ok((0..n)
                .map(|i| format!("{:?}", r.get::<_, Sql>(i).unwrap()))
                .collect::<Vec<_>>()
                .join("|"))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    let tasks_before = dump(c, "SELECT rowid, * FROM tasks ORDER BY rowid");
    let context_before = dump(c, "SELECT rowid, * FROM task_context ORDER BY rowid");

    // 0065 (T-1202, the paired phones), 0066 (APP-979, chat_history's copies), 0067 (MEM-436) and 0068 (PHN-828) follow
    assert_eq!(db.migrate().unwrap(), vec![64, 65, 66, 67, 68]);
    let c = db.conn();
    assert_eq!(
        dump(c, "SELECT rowid, * FROM tasks ORDER BY rowid"),
        tasks_before
    );
    assert_eq!(
        dump(c, "SELECT rowid, * FROM task_context ORDER BY rowid"),
        context_before
    );
    assert_eq!(
        dump(c, "SELECT name FROM sqlite_master WHERE tbl_name = 'tasks' AND type = 'index' ORDER BY name"),
        vec!["Text(\"sqlite_autoindex_tasks_1\")", "Text(\"tasks_status\")", "Text(\"tasks_tree\")"]
    );
    assert!(dump(c, "PRAGMA foreign_key_check").is_empty());
    assert_eq!(dump(c, "PRAGMA integrity_check"), vec!["Text(\"ok\")"]);
    assert!(dump(
        c,
        "SELECT name FROM sqlite_master WHERE name LIKE '%_hold' OR name = 'tasks_new'"
    )
    .is_empty());
    c.execute(
        "UPDATE tasks SET status = 'done_with_gaps' WHERE id = 't0'",
        [],
    )
    .unwrap();
    assert!(c
        .execute("UPDATE tasks SET status = 'nearly' WHERE id = 't0'", [])
        .is_err());
}
