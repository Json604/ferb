//! The bundled SQLite must be the one the Python daemon runs (3.53.4 or newer): FTS5's trigram tokenizer reads
//! text past a NUL there, and stopped at the NUL in 3.46 (journal entry 22).

use rusqlite::Connection;

#[test]
fn the_linked_sqlite_is_at_least_the_python_daemons() {
    assert!(
        rusqlite::version_number() >= 3_053_004,
        "bundled SQLite is {}",
        rusqlite::version()
    );
}

#[test]
fn the_trigram_tokenizer_reads_past_a_nul() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE t USING fts5(x, tokenize='trigram');
         CREATE VIRTUAL TABLE v USING fts5vocab(t, 'instance');",
    )
    .unwrap();
    conn.execute("INSERT INTO t (x) VALUES (?)", ["abc\0defg"])
        .unwrap();
    let terms: Vec<String> = conn
        .prepare("SELECT term FROM v ORDER BY offset")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        terms,
        ["abc", "bcd", "cde", "def", "efg"],
        "the NUL is skipped and the text after it is read (Python 3.53.4 does the same)"
    );
    let hits: i64 = conn
        .query_row("SELECT count(*) FROM t WHERE t MATCH 'efg'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(hits, 1);
}
