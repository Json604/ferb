//! The stored-data viewer's reads (STORE-423) on a real migrated store in a temporary folder, through the read-only
//! connection FERB.app's calls use: every table listed with its group, rows newest first in pages, a huge cell cut and
//! read in pieces, a search across tables (JSON-escaped words too), names only from sqlite_master, and no writes.

use std::time::Duration;

use ferb_store::data_view::{self as dv, Filter, RowsQuery};
use ferb_store::Db;
use rusqlite::params;
use serde_json::{json, Value};
use tempfile::TempDir;

fn store() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ferb.db");
    let db = Db::open(&path).unwrap();
    db.migrate().unwrap();
    let c = db.conn();
    for i in 0..7 {
        c.execute(
            "INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, 'chat.message', ?)",
            params![
                format!("j{i}"),
                format!("2026-11-15T10:0{i}:00+05:30"),
                format!("{{\"session_id\": \"owner\", \"request_id\": \"r{i}\", \"text\": \"message number {i}\"}}")
            ],
        )
        .unwrap();
    }
    c.execute(
        "INSERT INTO journal (id, at, kind, data_json) VALUES ('j-acc', '2026-11-15T11:00:00+05:30', 'chat.reply', ?)",
        // Python's json.dumps: the accented word stored as \u escapes, the quotes escaped
        params!["{\"session_id\": \"owner\", \"request_id\": \"r0\", \"text\": \"caf\\u00e9 said \\\"no\\\"\"}"],
    )
    .unwrap();
    // a model call's span with a long context
    let context = "x".repeat(250_000) + " the planner's words " + &"y".repeat(10);
    c.execute(
        "INSERT INTO spans (id, kind, started_at, input_json) VALUES ('s1', 'model.turn', '2026-11-15T10:00:00+05:30', ?)",
        params![serde_json::to_string(&json!({"role": "supervisor", "request_id": "r1", "context": context})).unwrap()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO spans (id, kind, started_at, input_json) VALUES ('s2', 'tool.call', '2026-11-15T10:00:01+05:30', '{}')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO memory_items (id, subject, key, content, created_at, updated_at) \
         VALUES ('m1', 'owner', 'tea', 'the owner likes green tea', '2026-11-15', '2026-11-15')",
        [],
    )
    .ok();
    drop(db);
    (dir, path)
}

fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

// decision: STORE-423
#[test]
fn every_table_is_listed_with_its_group_count_and_whether_it_is_internal() {
    let (_dir, path) = store();
    let conn = dv::open_read_only(&path).unwrap();
    let t = dv::tables(&conn).unwrap();
    let tables = t["tables"].as_array().unwrap();
    let listed = names(&t["tables"]);
    let mut in_master: Vec<String> = {
        let mut s = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    in_master.sort();
    assert_eq!(
        listed, in_master,
        "every table of the database, none invented"
    );
    let row = |name: &str| tables.iter().find(|t| t["name"] == name).unwrap().clone();
    assert_eq!(row("journal")["rows"], 8);
    assert_eq!(row("journal")["group"], "activity");
    assert_eq!(row("chat_entries")["group"], "conversation");
    assert_eq!(row("spans")["group"], "model_input");
    assert_eq!(row("memory_items")["group"], "memory");
    assert_eq!(row("confirmations")["group"], "safety");
    assert!(row("confirmations")["action"].is_null());
    assert!(row("confirmations")["note"]
        .as_str()
        .unwrap()
        .contains("Not deletable here"));
    assert_eq!(row("chat_history")["action"], "turn");
    assert_eq!(row("memory_items")["action"], "forget");
    // the full-text indexes and their shadow tables are internal; the tables they index are not
    for internal in [
        "chat_fts",
        "chat_fts_data",
        "chat_fts_idx",
        "chat_fts_config",
        "memory_fts",
    ] {
        assert_eq!(row(internal)["internal"], true, "{internal}");
    }
    assert_eq!(row("journal")["internal"], false);
    let groups: Vec<&str> = t["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        groups,
        [
            "conversation",
            "tasks",
            "model_input",
            "memory",
            "activity",
            "safety",
            "other"
        ]
    );
}

// decision: STORE-423
#[test]
fn rows_come_newest_first_in_pages_and_the_cursor_reads_the_next() {
    let (_dir, path) = store();
    let conn = dv::open_read_only(&path).unwrap();
    let q = |cursor| RowsQuery {
        table: "journal".into(),
        cursor,
        limit: 3,
        cell: 40,
        filter: None,
    };
    let first = dv::rows(&conn, &q(None)).unwrap();
    let ids = |page: &Value| -> Vec<String> {
        page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["cells"][1].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(first["columns"][1], "id");
    assert_eq!(ids(&first), ["j-acc", "j6", "j5"]);
    assert_eq!(first["total"], 8);
    let second = dv::rows(&conn, &q(first["next"].as_i64())).unwrap();
    assert_eq!(ids(&second), ["j4", "j3", "j2"]);
    let third = dv::rows(&conn, &q(second["next"].as_i64())).unwrap();
    assert_eq!(ids(&third), ["j1", "j0"]);
    assert!(third.get("next").is_none(), "the last page has no next");
    // a turn's conversation row carries the action that deletes its turn
    assert_eq!(
        first["rows"][1]["action"],
        json!({"do": "turn", "id": "r6"})
    );
    // a filter on a column the table has
    let spans = dv::rows(
        &conn,
        &RowsQuery {
            table: "spans".into(),
            cursor: None,
            limit: 10,
            cell: 100,
            filter: Some(Filter {
                column: "kind".into(),
                equals: None,
                starts_with: Some("model.".into()),
            }),
        },
    )
    .unwrap();
    assert_eq!(spans["total"], 1);
    assert_eq!(spans["rows"].as_array().unwrap().len(), 1);
}

// decision: STORE-423
#[test]
fn a_huge_cell_is_cut_and_read_in_pieces() {
    let (_dir, path) = store();
    let conn = dv::open_read_only(&path).unwrap();
    let page = dv::rows(
        &conn,
        &RowsQuery {
            table: "spans".into(),
            cursor: None,
            limit: 10,
            cell: 100,
            filter: None,
        },
    )
    .unwrap();
    let s1 = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["cells"][0] == "s1")
        .unwrap();
    let input_at = page["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c == "input_json")
        .unwrap();
    assert_eq!(s1["cells"][input_at].as_str().unwrap().chars().count(), 100);
    let full = s1["cut"]["input_json"].as_i64().unwrap();
    assert!(full > 250_000);
    // one row in full: the long cell is still cut at ROW_CELL, with its length
    let rowid = s1["rowid"].as_i64().unwrap();
    let row = dv::row(&conn, "spans", rowid).unwrap();
    assert_eq!(
        row["cells"][input_at].as_str().unwrap().chars().count(),
        dv::ROW_CELL
    );
    assert_eq!(row["cut"]["input_json"], full);
    // the rest in pieces, to its end
    let mut offset = dv::ROW_CELL;
    let mut text = row["cells"][input_at].as_str().unwrap().to_string();
    loop {
        let piece = dv::cell(&conn, "spans", rowid, "input_json", offset, dv::PIECE).unwrap();
        let t = piece["text"].as_str().unwrap();
        text.push_str(t);
        offset += t.chars().count();
        if piece["end"] == true {
            break;
        }
    }
    assert_eq!(text.chars().count() as i64, full);
    assert!(text.contains("the planner's words"));
    assert!(dv::cell(&conn, "spans", rowid, "input_json", 0, dv::PIECE_MAX + 1).is_err());
}

// decision: STORE-423
#[test]
fn a_search_finds_a_phrase_across_tables_and_inside_json() {
    let (_dir, path) = store();
    let conn = dv::open_read_only(&path).unwrap();
    let found = dv::search(&conn, "Message Number 3", None, Duration::from_secs(30)).unwrap();
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["table"], "journal");
    assert_eq!(hits[0]["column"], "data_json");
    assert!(hits[0]["snippet"]
        .as_str()
        .unwrap()
        .contains("message number 3"));
    assert_eq!(found["stopped"], false);
    // stored as Python's JSON writes it (é, \"), found from the words as typed
    let accented = dv::search(&conn, "café said \"no\"", None, Duration::from_secs(30)).unwrap();
    assert_eq!(accented["hits"][0]["table"], "journal", "{accented}");
    // words deep inside a 250 000-character cell, in the "What the AI was given" group
    let deep = dv::search(&conn, "the planner's words", None, Duration::from_secs(30)).unwrap();
    assert_eq!(deep["hits"][0]["table"], "spans");
    assert_eq!(deep["hits"][0]["group"], "model_input");
    // a LIKE wildcard in the phrase is a character, not a wildcard
    let none = dv::search(&conn, "message%3", None, Duration::from_secs(30)).unwrap();
    assert!(none["hits"].as_array().unwrap().is_empty());
    // a spent budget stops the search and says so
    let stopped = dv::search(&conn, "message", None, Duration::ZERO).unwrap();
    assert_eq!(stopped["stopped"], true);
}

// decision: STORE-423
#[test]
fn names_come_only_from_the_database_and_nothing_is_written() {
    let (_dir, path) = store();
    let conn = dv::open_read_only(&path).unwrap();
    let q = |table: &str| RowsQuery {
        table: table.into(),
        cursor: None,
        limit: 5,
        cell: 50,
        filter: None,
    };
    for bad in [
        "journal; DROP TABLE journal",
        "\"journal\"",
        "nope",
        "sqlite_master",
    ] {
        assert!(dv::rows(&conn, &q(bad)).is_err(), "{bad}");
    }
    let bad_column = RowsQuery {
        filter: Some(Filter {
            column: "kind) OR 1=1 --".into(),
            equals: Some("x".into()),
            starts_with: None,
        }),
        ..q("journal")
    };
    assert!(dv::rows(&conn, &bad_column).is_err());
    assert!(dv::search(
        &conn,
        "x",
        Some(&["no_such".to_string()]),
        Duration::from_secs(5)
    )
    .is_err());
    assert!(dv::rows(
        &conn,
        &RowsQuery {
            limit: 0,
            ..q("journal")
        }
    )
    .is_err());
    assert!(dv::rows(
        &conn,
        &RowsQuery {
            cell: dv::CELL_MAX + 1,
            ..q("journal")
        }
    )
    .is_err());
    // the viewer's connection can't write
    assert!(conn.execute("DELETE FROM journal", []).is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM journal", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        8
    );
}
