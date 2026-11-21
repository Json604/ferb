//! The start's database check never moves a database it isn't sure is damaged (decisions: STORE-429,
//! STORE-430, STORE-431). On 2026-11-21 FERB set the owner's healthy database aside twice: `ferb setup`
//! checked it while the daemon was writing it, and FTS5's own check, run as the second statement of
//! one connection, read the first statement's segment list against newer rows. Temporary folders
//! only (TEST-508).

use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use ferb_store::{
    pytime::DateTime,
    salvage::{self, Seams},
    Db,
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

fn now() -> DateTime {
    DateTime::new(2026, 11, 21, 10, 12, 35, 0, None).unwrap()
}

fn no_toml_error(_: &str) -> Option<String> {
    None
}

/// A migrated store in `home` whose trail (and so trail_fts, by its triggers) holds `rows` rows,
/// written in many transactions so the full-text index has several segments.
fn store(home: &Path, rows: usize) -> PathBuf {
    let path = home.join(salvage::STORE);
    let db = Db::open(&path).unwrap();
    db.migrate().unwrap();
    for chunk in 0..rows.div_ceil(10) {
        db.transaction(|c| -> rusqlite::Result<()> {
            for i in chunk * 10..(chunk * 10 + 10).min(rows) {
                c.execute(
                    "INSERT INTO trail(id, at, day, device_id, actor, source, data_json) VALUES (?1, '2026-11-21T10:00:00+05:30', '2026-11-21', 'd', 'owner', 'app', ?2)",
                    rusqlite::params![
                        format!("t{i}"),
                        format!(r#"{{"title": "window {i} notes mail word{}"}}"#, i % 37)
                    ],
                )?;
            }
            Ok(())
        })
        .unwrap();
    }
    db.close().unwrap();
    path
}

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn pragma(conn: &Connection, sql: &str) -> Vec<String> {
    let mut statement = conn.prepare(sql).unwrap();
    let rows = statement.query_map([], |r| r.get(0)).unwrap();
    rows.collect::<rusqlite::Result<_>>().unwrap()
}

fn trail_rows(path: &Path) -> i64 {
    read_only(path)
        .query_row("SELECT count(*) FROM trail", [], |r| r.get(0))
        .unwrap()
}

/// Every file in `home`, by name.
fn files(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(home)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// decision: STORE-430
#[test]
fn fts5_reads_a_stale_segment_list_in_a_connections_second_check() {
    // The reproduction, at SQLite's level: a writer merges the index between two checks on one
    // read-only connection; the second reports corruption, a new connection finds none. If a
    // future SQLite stops doing this, the first assertion fails: the cause is then gone, and the
    // one-connection-per-check rule stays harmless.
    let tmp = TempDir::new().unwrap();
    let path = store(tmp.path(), 400);
    let reader = read_only(&path);
    assert_eq!(pragma(&reader, "PRAGMA quick_check"), ["ok"]);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("INSERT INTO trail_fts(trail_fts) VALUES('optimize')")
        .unwrap();
    let stale = pragma(&reader, "PRAGMA integrity_check");
    assert!(
        stale.iter().any(|line| line.contains("fts5:")),
        "the second check on one connection: {stale:?}"
    );
    assert_eq!(pragma(&read_only(&path), "PRAGMA integrity_check"), ["ok"]);
    // salvage's own check opens a connection per check: the same file is not damaged
    assert_eq!(salvage::damage(&path, true).unwrap(), None);
}

// decision: STORE-430
#[test]
fn a_database_written_while_it_is_checked_is_never_set_aside() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let path = store(&home, 2000);
    let before = trail_rows(&path);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let pauses = Cell::new(0);
    let pause = |_: Duration| pauses.set(pauses.get() + 1);
    let seams = Seams {
        now: &now,
        toml_error: &no_toml_error,
        pause: &pause,
        rebuilt: &|tables, why| panic!("nothing to rebuild: {tables:?} {why}"),
    };
    std::thread::scope(|scope| {
        // another process's writes, as the daemon's sensing writes the trail: rows in and out,
        // and the index merged
        scope.spawn(|| {
            let writer = Connection::open(&path).unwrap();
            writer.busy_timeout(Duration::from_secs(5)).unwrap();
            let mut n = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                n += 1;
                writer
                    .execute(
                        "INSERT INTO trail(id, at, day, device_id, actor, source, data_json) VALUES (?1, 'a', 'd', 'd', 'owner', 'app', ?2)",
                        rusqlite::params![format!("w{n}"), format!(r#"{{"t": "busy word{n} other{}"}}"#, n % 7)],
                    )
                    .unwrap();
                if n % 5 == 0 {
                    writer
                        .execute("DELETE FROM trail WHERE seq = (SELECT min(seq) FROM trail)", [])
                        .unwrap();
                }
                if n % 20 == 0 {
                    writer
                        .execute_batch("INSERT INTO trail_fts(trail_fts, rank) VALUES('merge', 16)")
                        .unwrap();
                }
            }
        });
        // the writer stops whatever the checks say, so a failure is reported, never a hang
        let checks: Vec<_> = ["b1", "b2", "b3", "b4", "b5"]
            .iter()
            .map(|build| (build, salvage::check_store(&home, build, &seams)))
            .collect();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for (build, moved) in checks {
            let moved = moved.unwrap();
            assert!(moved.is_empty(), "{build}: set aside {moved:?}");
        }
    });
    assert!(path.is_file());
    assert!(trail_rows(&path) > before / 2);
    assert!(salvage::pending(&home).is_empty());
}

// decision: STORE-430
#[test]
fn damage_not_found_again_after_the_pause_is_not_damage() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let path = store(&home, 50);
    let good = fs::read(&path).unwrap();
    let broken = broken_btree(&path);
    fs::write(&path, &broken).unwrap();
    assert!(salvage::damage(&path, false).unwrap().is_some());
    // what the first check saw is gone by the second: the file is kept and the build marked checked
    let waited: RefCell<Vec<Duration>> = RefCell::default();
    let pause = |wait: Duration| {
        waited.borrow_mut().push(wait);
        fs::write(&path, &good).unwrap();
    };
    let seams = Seams {
        now: &now,
        toml_error: &no_toml_error,
        pause: &pause,
        rebuilt: &|_, _| panic!("nothing to rebuild"),
    };
    assert!(salvage::check_store(&home, "b1", &seams)
        .unwrap()
        .is_empty());
    assert_eq!(*waited.borrow(), [salvage::RECHECK_AFTER]);
    assert!(path.is_file());
    assert!(salvage::pending(&home).is_empty());
    let checked = fs::read_to_string(home.join(salvage::CHECKED)).unwrap();
    assert!(checked.contains("\"b1\""), "{checked}");
}

// decision: STORE-430, STORE-1
#[test]
fn damage_found_again_outside_the_indexes_is_still_set_aside() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let path = store(&home, 50);
    let broken = broken_btree(&path);
    fs::write(&path, &broken).unwrap();
    let pauses = Cell::new(0);
    let pause = |_: Duration| pauses.set(pauses.get() + 1);
    let seams = Seams {
        now: &now,
        toml_error: &no_toml_error,
        pause: &pause,
        rebuilt: &|_, _| panic!("a damaged table is never rebuilt"),
    };
    let moved = salvage::check_store(&home, "", &seams).unwrap();
    assert_eq!(pauses.get(), 1);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0]["file"], "ferb.db");
    assert!(moved[0]["why"]
        .as_str()
        .unwrap()
        .starts_with("SQLite's integrity check: "));
    assert!(!path.exists());
    assert_eq!(
        fs::read(home.join("ferb.db.broken-20261121-1012")).unwrap(),
        broken
    );
    assert_eq!(salvage::pending(&home).len(), 1);
}

// decision: STORE-431
#[test]
fn a_damaged_search_index_is_rebuilt_in_place_never_set_aside() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let path = store(&home, 300);
    {
        // the index and the text it was built from disagree (FTS5's "malformed inverted index")
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE trail_fts_content SET c0 = 'zebra' WHERE id = (SELECT min(id) FROM trail_fts_content)",
            [],
        )
        .unwrap();
    }
    let found = salvage::damage(&path, true).unwrap().unwrap();
    assert!(found.contains("trail_fts"), "{found}");
    let told: RefCell<Vec<(Vec<String>, String)>> = RefCell::default();
    let rebuilt =
        |tables: &[String], why: &str| told.borrow_mut().push((tables.to_vec(), why.to_owned()));
    let seams = Seams {
        now: &now,
        toml_error: &no_toml_error,
        pause: &|_| {},
        rebuilt: &rebuilt,
    };
    let moved = salvage::check_store(&home, "b1", &seams).unwrap();
    assert!(moved.is_empty(), "{moved:?}");
    assert_eq!(*told.borrow(), [(vec!["trail_fts".to_owned()], found)]);
    assert_eq!(
        files(&home).iter().filter(|f| f.contains("broken")).count(),
        0
    );
    assert!(salvage::pending(&home).is_empty());
    assert_eq!(salvage::damage(&path, true).unwrap(), None);
    assert_eq!(trail_rows(&path), 300);
    // the rebuilt index finds what the text holds
    let hits: i64 = read_only(&path)
        .query_row(
            "SELECT count(*) FROM trail_fts WHERE trail_fts MATCH 'zebra'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1);
    assert!(fs::read_to_string(home.join(salvage::CHECKED))
        .unwrap()
        .contains("\"b1\""));
}

// decision: STORE-431
#[test]
fn a_search_index_missing_its_pages_is_rebuilt_too() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let path = store(&home, 300);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute("DELETE FROM trail_fts_data WHERE id > 10", [])
            .unwrap();
    }
    let found = salvage::damage(&path, true).unwrap().unwrap();
    assert!(
        found.contains("fts5: corruption found reading blob"),
        "{found}"
    );
    let told = Cell::new(0);
    let seams = Seams {
        now: &now,
        toml_error: &no_toml_error,
        pause: &|_| {},
        rebuilt: &|_, _| told.set(told.get() + 1),
    };
    assert!(salvage::check_store(&home, "", &seams).unwrap().is_empty());
    assert_eq!(told.get(), 1);
    assert_eq!(salvage::damage(&path, true).unwrap(), None);
    assert_eq!(trail_rows(&path), 300);
}

// decision: STORE-429, STORE-3
#[test]
fn setup_never_checks_or_moves_the_database() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    let path = store(&home, 50);
    let broken = broken_btree(&path);
    fs::write(&path, &broken).unwrap();
    let seams = Seams {
        now: &now,
        toml_error: &no_toml_error,
        pause: &|_| panic!("setup doesn't check the database"),
        rebuilt: &|_, _| panic!("setup doesn't check the database"),
    };
    assert!(salvage::check_setup(&home, &seams).unwrap().is_empty());
    assert_eq!(fs::read(&path).unwrap(), broken);
    assert!(!home.join(salvage::CHECKED).exists());
    // a newer FERB's database still refuses the setup, unchanged
    let tmp = TempDir::new().unwrap();
    let path = store(tmp.path(), 0);
    Connection::open(&path)
        .unwrap()
        .execute("INSERT INTO schema_version(version) VALUES (9999)", [])
        .unwrap();
    let err = salvage::check_setup(tmp.path(), &seams).unwrap_err();
    assert!(
        matches!(err, ferb_store::Error::NewerStore { newest: 9999, .. }),
        "{err}"
    );
}

/// `path`'s bytes with the root page of the trail's day index overwritten: damage SQLite's check
/// reports as lines about that tree, in no full-text index.
fn broken_btree(path: &Path) -> Vec<u8> {
    let conn = read_only(path);
    let root: i64 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'trail_day'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let size: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    let mut bytes = fs::read(path).unwrap();
    let at = ((root - 1) * size) as usize;
    bytes[at + 12..at + 40].fill(0xff);
    bytes
}
