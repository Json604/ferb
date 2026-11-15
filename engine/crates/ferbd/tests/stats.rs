//! `ferb stats` against what Python's `ferb.cli.admin.stats` printed (engine/fixtures/stats/make.py): each case
//! builds the same FERB home (no database, an empty file, or a store migrated from the same files and filled with the
//! same rows, some tables dropped), runs with the same instant and `--days`, and must print the same bytes and exit
//! with the same status.

use std::path::{Path, PathBuf};

use ferb_py::time::DateTime;
use ferbd::stats::{stats, Now};
use rusqlite::types::Value as Sql;
use serde_json::Value;

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stats/stats.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture")).expect("JSON")
}

fn sql(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
        Value::Number(n) => Sql::Real(n.as_f64().unwrap()),
        Value::String(s) => Sql::Text(s.clone()),
        other => panic!("not a parameter: {other}"),
    }
}

/// The case's FERB home, as the fixture built it.
fn build(case: &Value, home: &Path) {
    std::fs::create_dir_all(home).unwrap();
    let path = home.join("ferb.db");
    match case["db"].as_str().unwrap() {
        "none" => {}
        "empty_file" => std::fs::write(&path, b"").unwrap(),
        "migrated" => {
            let db = ferb_store::Db::open(&path).unwrap();
            db.migrate().unwrap();
            let c = db.conn();
            c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            for row in case["rows"].as_array().unwrap() {
                let params: Vec<Sql> = row[1].as_array().unwrap().iter().map(sql).collect();
                c.execute(
                    row[0].as_str().unwrap(),
                    rusqlite::params_from_iter(params.iter()),
                )
                .unwrap();
            }
            for table in case["drop"].as_array().unwrap() {
                c.execute_batch(&format!("DROP TABLE {}", table.as_str().unwrap()))
                    .unwrap();
            }
            db.close().unwrap();
        }
        other => panic!("unknown db {other}"),
    }
}

// decision: DMN-4
#[test]
fn every_case_prints_what_python_printed() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut differ = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let home = dir.path().join(name);
        build(case, &home);
        let now = Now {
            utc: DateTime::from_isoformat(case["now_utc"].as_str().unwrap()).unwrap(),
            local: DateTime::from_isoformat(case["now_local"].as_str().unwrap()).unwrap(),
        };
        let mut out = Vec::new();
        let status = stats(&home, &mut out, case["days"].as_i64().unwrap(), now);
        let got = String::from_utf8(out).unwrap();
        let want = case["stdout"]
            .as_str()
            .unwrap()
            .replace("{home}", home.to_str().unwrap());
        let want_status = case["status"].as_i64().unwrap() as i32;
        if got != want || status != want_status {
            eprintln!(
                "case {name}: status {status} (want {want_status})\n--- got\n{got}--- want\n{want}"
            );
            differ.push(name.to_owned());
        }
    }
    eprintln!("{} cases, {} differ", cases.len(), differ.len());
    assert!(differ.is_empty(), "cases that differ: {differ:?}");
    assert!(cases.len() >= 20);
}
