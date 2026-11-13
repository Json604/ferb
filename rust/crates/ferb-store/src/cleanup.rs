//! The one-time clean-up of data no code uses any more: a port of `ferb/store/cleanup.py`
//! (decisions: STORE-402, STORE-411, STORE-412, STORE-416). Run by setup with the daemon
//! stopped, only after the owner's OK on exactly what it removes. It never touches memory,
//! episodes, chat, tasks, schedules or traces still in their window (`PROTECTED`).
//!
//! The owner's OK is the preview's `ok` digest: `apply` reads the plan again and refuses,
//! deleting nothing, when it differs from what the owner saw.
//!
//! Seams: the episode cards' source plan and its pruning belong to learning
//! (`ferb/memory/learning.py:687,725`) and come in as `CardSources`; the journal
//! (`ferb/memory/journal.py`) as `JournalSink`.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    pyjson::{self, Style},
    pyval::get,
    store_size, Db, Result,
};

/// journal: what a run removed (counts only) (`cleanup.py:32`).
pub const CLEANED: &str = "store.cleaned";
/// journal: the compaction (sizes only) (`cleanup.py:33`).
pub const COMPACTED: &str = "store.compacted";
pub const ROLLUP_MAKER: &str = "code:rollup/1";
/// Rows a clean-up transaction takes (`cleanup.py:36`).
pub const BATCH: usize = 5000;
/// (table, what it held): emptied whole, schema kept (`cleanup.py:38-47`).
pub const EMPTIED: [(&str, &str); 8] = [
    ("decisions", "Jev's decisions (Jev is switched off)"),
    (
        "perception_receipts",
        "perception batch receipts (Jev is switched off)",
    ),
    (
        "perception_batches",
        "perception batches (Jev is switched off)",
    ),
    (
        "perception_labels",
        "perception labels (Jev is switched off)",
    ),
    (
        "notification_told",
        "notification receipts (Jev is switched off)",
    ),
    (
        "learning_vectors",
        "the old embedding model's vectors (replaced in step 9b)",
    ),
    (
        "learning_embedding_evals",
        "the old embedding model's evaluations (replaced in step 9b)",
    ),
    (
        "task_memory_refs",
        "the memory refs tasks were given (no longer written or read)",
    ),
];
/// (table, what it held): no code reads or writes it any more; the run drops the table itself
/// (`cleanup.py:49-54`; decision: STORE-411).
pub const DROPPED: [(&str, &str); 4] = [
    ("owner_questions", "folder-access questions (never used)"),
    (
        "notification_rules",
        "notification rules (removed in step 3)",
    ),
    (
        "learning_receipts",
        "procedure receipts (learning removed in step 3)",
    ),
    (
        "learning_jobs",
        "learning job states (learning removed in step 3)",
    ),
];
/// Tables of `EMPTIED` whose rows the hourly retention pass also trims while the daemon runs:
/// their steps are `live` (`cleanup.py:57`).
pub const TRIMMED: [&str; 2] = ["decisions", "perception_receipts"];
/// Never written by a run: what STORE-402 protects and what FERB keeps forever
/// (`cleanup.py:59-63`).
pub const PROTECTED: [&str; 33] = [
    "memory_items",
    "memory_sources",
    "memory_profile",
    "memory_context",
    "memory_set_aside",
    "memory_import_runs",
    "memory_import_done",
    "memory_import_items",
    "memory_forgotten",
    "forgotten_refs",
    "episodes",
    "episode_how",
    "journal",
    "tasks",
    "task_results",
    "task_result_cards",
    "task_result_names",
    "task_context",
    "schedules",
    "schedule_ends",
    "schedule_names",
    "timers",
    "timer_names",
    "conversations",
    "owner_verdicts",
    "spans",
    "trail",
    "known_destinations",
    "standing_permissions",
    "recall_vectors",
    "chat_fts",
    "learning_nodes",
    "chat_history",
];
/// The protected rows of a table a run also removes from (`cleanup.py:66`): every learned node
/// but the old rollups.
pub const PROTECTED_ROWS: [(&str, &str); 1] =
    [("learning_nodes", "made_by IS NOT 'code:rollup/1'")];

/// The episode cards' side of the clean-up (`ferb/memory/learning.py`), owned by learning.
pub trait CardSources {
    /// `Learning.card_source_plan(removing=...)` (`learning.py:687`), read without writing: an
    /// object with `drop` (node id → the refs it loses), `tasks`, `nodes`, `untraced`,
    /// `stopped`. `removing`: learned nodes the same clean-up removes, read as already gone.
    fn card_source_plan(&self, conn: &Connection, removing: &[String]) -> Result<Value>;
    /// `Learning.prune_card_sources(stop=..., plan=...)` (`learning.py:725`): removes the
    /// planned rows; an object with at least `rows` and `stopped`.
    fn prune_card_sources(
        &self,
        conn: &Connection,
        stop: &dyn Fn() -> bool,
        plan: &Value,
    ) -> Result<Value>;
}

/// `Journal.append(kind, data)` (`ferb/memory/journal.py`), as the clean-up uses it.
pub trait JournalSink {
    fn append(&mut self, kind: &str, data: &Value) -> Result<()>;
}

fn count(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<i64> {
    Ok(conn.query_row(sql, params, |r| r.get(0))?)
}

fn step(table: &str, rows: i64, what: &str) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("table".to_owned(), json!(table));
    out.insert("rows".to_owned(), json!(rows));
    out.insert("what".to_owned(), json!(what));
    out
}

fn is(step: &Value, flag: &str) -> bool {
    get(step, flag).is_some_and(crate::pyval::truthy)
}

/// `plan` (`cleanup.py:79-120`; decision: STORE-412): what a run would remove, read without
/// writing (any connection, a read-only one included): `steps` (table, rows, what; `live` when
/// the running daemon's hourly pass also changes its rows), `compact` (the file still needs its
/// one-time compaction), `rows` in all, `ok` (the digest the owner's OK carries) and `cards`,
/// the card plan `apply` follows.
pub fn plan(conn: &Connection, cards: &dyn CardSources) -> Result<Value> {
    let mut steps: Vec<Value> = Vec::new();
    for (table, what) in EMPTIED {
        let n = count(conn, &format!("SELECT COUNT(*) FROM {table}"), [])?;
        if n != 0 {
            let mut s = step(table, n, what);
            if TRIMMED.contains(&table) {
                s.insert("live".to_owned(), json!(true));
            }
            steps.push(Value::Object(s));
        }
    }
    // activity rows age out hourly
    let n = count(
        conn,
        "SELECT COUNT(*) FROM trail WHERE relevance IS NOT NULL",
        [],
    )?;
    if n != 0 {
        let mut s = step(
            "trail.relevance",
            n,
            "Jev's relevance labels on activity rows (only the label; the rows stay)",
        );
        s.insert("live".to_owned(), json!(true));
        steps.push(Value::Object(s));
    }
    let n = count(
        conn,
        "SELECT COUNT(*) FROM learning_nodes WHERE made_by = ?",
        [ROLLUP_MAKER],
    )?;
    if n != 0 {
        let sources = count(
            conn,
            "SELECT COUNT(*) FROM learning_sources WHERE node_id IN (SELECT id FROM learning_nodes \
             WHERE made_by = ?)",
            [ROLLUP_MAKER],
        )?;
        let changes = count(
            conn,
            "SELECT COUNT(*) FROM learning_changes WHERE node_id IN (SELECT id FROM learning_nodes \
             WHERE made_by = ?)",
            [ROLLUP_MAKER],
        )?;
        steps.push(Value::Object(step(
            "learning_nodes",
            n,
            "day/week/month rollups (removed in step 3)",
        )));
        if sources != 0 {
            steps.push(Value::Object(step(
                "learning_sources",
                sources,
                "the rollups' source rows",
            )));
        }
        if changes != 0 {
            steps.push(Value::Object(step(
                "learning_changes",
                changes,
                "the rollups' change history",
            )));
        }
    }
    let rollups: Vec<String> = {
        let mut statement = conn.prepare("SELECT id FROM learning_nodes WHERE made_by = ?")?;
        let rows = statement.query_map([ROLLUP_MAKER], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // as they will be once the rollups are gone
    let cards = cards.card_source_plan(conn, &rollups)?;
    let empty = Map::new();
    let drop = get(&cards, "drop")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let n: usize = drop
        .values()
        .map(|refs| refs.as_array().map_or(0, Vec::len))
        .sum();
    if n != 0 {
        let nodes = get(&cards, "nodes").map_or_else(|| "None".to_owned(), crate::pyval::py_str);
        let mut s = step(
            "learning_sources",
            n as i64,
            &format!(
                "episode cards' source rows the current card rule no longer gives them \
                 ({} of {nodes} card versions)",
                drop.len()
            ),
        );
        // new cards, and traces aging out, change these counts
        s.insert("live".to_owned(), json!(true));
        steps.push(Value::Object(s));
    }
    let names: Vec<String> = {
        let mut statement = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
        let rows = statement.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (table, what) in DROPPED {
        if names.iter().any(|n| n == table) {
            let rows = count(conn, &format!("SELECT COUNT(*) FROM {table}"), [])?;
            let mut s = step(table, rows, &format!("{what}; the table is dropped"));
            s.insert("drop".to_owned(), json!(true));
            steps.push(Value::Object(s));
        }
    }
    let compact = count(conn, "PRAGMA auto_vacuum", [])? != 2;
    // What the OK approves: every step and the compaction, with the exact rows of each step
    // nothing else changes; a `live` step by its table only
    let approved_steps: Vec<Value> = steps
        .iter()
        .map(|s| {
            if is(s, "live") {
                json!({"table": s["table"], "live": true})
            } else {
                s.clone()
            }
        })
        .collect();
    let approved = json!({"steps": approved_steps, "compact": compact});
    let text = pyjson::dumps_with(
        &approved,
        Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    );
    let ok = hex::encode(Sha256::digest(text.as_bytes()))[..16].to_owned();
    let rows: i64 = steps.iter().filter_map(|s| s["rows"].as_i64()).sum();
    Ok(json!({"steps": steps, "compact": compact, "rows": rows, "ok": ok, "cards": cards}))
}

fn read_only(path: &Path) -> rusqlite::Result<Connection> {
    // Python opens `file:<path>?mode=ro`: read-only, never created.
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// `preview` (`cleanup.py:123-141`): `plan` on a read-only connection (a daemon may be
/// running), with the file's size (`bytes`), the free disk the compaction needs (`needed`;
/// decision: STORE-416) and the disk's free space (`free`), without the card plan: what the app
/// shows the owner.
pub fn preview(
    path: &Path,
    cards: &dyn CardSources,
    free: impl FnOnce(&Path) -> std::io::Result<u64>,
) -> Result<Value> {
    let conn = read_only(path)?;
    let mut out = plan(&conn, cards)?;
    let pragma = |name: &str| count(&conn, &format!("PRAGMA {name}"), []);
    let used = pragma("page_count")? - pragma("freelist_count")?;
    let needed = ((used * pragma("page_size")?) as f64 * crate::COMPACT_ROOM) as u64;
    drop(conn);
    let map = out.as_object_mut().expect("a plan is an object");
    map.shift_remove("cards");
    map.insert("bytes".to_owned(), json!(store_size(path)));
    map.insert("needed".to_owned(), json!(needed));
    map.insert(
        "free".to_owned(),
        json!(free(path.parent().unwrap_or(Path::new(".")))?),
    );
    Ok(out)
}

/// `pending` (`cleanup.py:144-166`): whether the clean-up has something to do, cheaply
/// (doctor): data of switched-off or removed code (a dropped table's rows included; an empty
/// one waits), rollups, or a file not compacted yet. Read-only; a store it can't read has
/// nothing to do.
pub fn pending(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let check = || -> rusqlite::Result<bool> {
        let conn = read_only(path)?;
        if conn.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0))? != 2 {
            return Ok(true);
        }
        let names: Vec<String> = {
            let mut statement =
                conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
            let rows = statement.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let dropped = DROPPED.iter().filter(|(t, _)| names.iter().any(|n| n == t));
        for (table, _) in EMPTIED.iter().chain(dropped) {
            let any: bool =
                conn.query_row(&format!("SELECT EXISTS (SELECT 1 FROM {table})"), [], |r| {
                    r.get(0)
                })?;
            if any {
                return Ok(true);
            }
        }
        conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM learning_nodes WHERE made_by = ?)",
            [ROLLUP_MAKER],
            |r| r.get(0),
        )
    };
    check().unwrap_or(false)
}

/// What `apply` is told and asked while it runs (`cleanup.py:169-171`).
pub struct Run<'a> {
    /// Asked between batches; true ends the run there (journaled with `stopped`).
    pub stop: &'a dyn Fn() -> bool,
    /// Told each step's line as it starts.
    pub say: &'a dyn Fn(&str),
    /// The free bytes of a folder's volume (`disk_free`).
    pub free: &'a dyn Fn(&Path) -> std::io::Result<u64>,
}

struct Removed(Map<String, Value>);

impl Removed {
    fn add(&mut self, label: &str, n: i64) {
        let had = self.0.get(label).and_then(Value::as_i64).unwrap_or(0);
        self.0.insert(label.to_owned(), json!(had + n));
    }
}

/// Run `sql` (a statement touching at most `BATCH` rows) until it touches fewer; false when
/// stopped (`cleanup.py:183-198`).
fn batches(
    conn: &Connection,
    removed: &mut Removed,
    stop: &dyn Fn() -> bool,
    label: &str,
    sql: &str,
) -> Result<bool> {
    loop {
        if stop() {
            return Ok(false);
        }
        let n = in_transaction(conn, || Ok(conn.execute(sql, [])?))?;
        removed.add(label, n as i64);
        if n < BATCH {
            return Ok(true);
        }
    }
}

fn in_transaction<T>(conn: &Connection, block: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let done = block().and_then(|value| {
        conn.execute_batch("COMMIT")?;
        Ok(value)
    });
    if done.is_err() && !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    done
}

/// `apply` (`cleanup.py:169-261`; decisions: STORE-402, STORE-412): the clean-up itself, on a
/// connection nothing else shares (setup, the daemon stopped). Refused (`refused`, nothing
/// written) unless `ok` is the digest of the plan read now. Each step deletes in batches;
/// `stop`, asked between batches, ends the run there. Then the compaction, when the disk has
/// room. Returns `removed` (rows by step), `dropped`, `stopped`, and the compaction's result.
pub fn apply(
    db: &Db,
    journal: &mut dyn JournalSink,
    cards: &dyn CardSources,
    ok: &str,
    run: Run<'_>,
) -> Result<Value> {
    let conn = db.conn();
    let mut now = plan(conn, cards)?;
    if now["ok"].as_str() != Some(ok) {
        now.as_object_mut()
            .expect("a plan is an object")
            .shift_remove("cards");
        return Ok(json!({"refused": true, "plan": now}));
    }
    let mut removed = Removed(Map::new());
    let mut dropped: Vec<String> = Vec::new();
    let (stop, say) = (run.stop, run.say);

    let steps = |removed: &mut Removed, dropped: &mut Vec<String>| -> Result<bool> {
        let card_plan = &now["cards"];
        let drop = get(card_plan, "drop").and_then(Value::as_object);
        if let Some(drop) = drop.filter(|d| !d.is_empty()) {
            // first: the deletes that take longest, stoppable between cards
            say(&format!(
                "Pruning the source rows of {} episode card versions\u{2026}",
                drop.len()
            ));
            let counts = cards.prune_card_sources(conn, stop, card_plan)?;
            removed.0.insert(
                "learning_sources (cards)".to_owned(),
                counts.get("rows").cloned().unwrap_or(json!(0)),
            );
            if is(&counts, "stopped") {
                return Ok(false);
            }
        }
        for (table, _) in EMPTIED {
            say(&format!("Emptying {table}\u{2026}"));
            let sql = format!(
                "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT {BATCH})"
            );
            if !batches(conn, removed, stop, table, &sql)? {
                return Ok(false);
            }
        }
        let sql = format!(
            "UPDATE trail SET relevance = NULL WHERE rowid IN (SELECT rowid FROM trail \
             WHERE relevance IS NOT NULL LIMIT {BATCH})"
        );
        if !batches(conn, removed, stop, "trail.relevance", &sql)? {
            return Ok(false);
        }
        say("Removing the old rollup nodes\u{2026}");
        let rollups = format!("SELECT id FROM learning_nodes WHERE made_by = '{ROLLUP_MAKER}'");
        let sql = format!(
            "DELETE FROM learning_changes WHERE rowid IN (SELECT rowid FROM \
             learning_changes WHERE node_id IN ({rollups}) LIMIT {BATCH})"
        );
        if !batches(conn, removed, stop, "learning_changes (rollups)", &sql)? {
            return Ok(false);
        }
        let before = count(conn, "SELECT COUNT(*) FROM learning_sources", [])?;
        let sql = format!(
            "DELETE FROM learning_nodes WHERE rowid IN (SELECT rowid FROM \
             learning_nodes WHERE made_by = '{ROLLUP_MAKER}' LIMIT {BATCH})"
        );
        if !batches(conn, removed, stop, "learning_nodes (rollups)", &sql)? {
            return Ok(false);
        }
        let after = count(conn, "SELECT COUNT(*) FROM learning_sources", [])?;
        removed.0.insert(
            "learning_sources (rollups)".to_owned(),
            json!(before - after),
        );
        for step in now["steps"].as_array().into_iter().flatten() {
            if !is(step, "drop") {
                continue;
            }
            if stop() {
                return Ok(false);
            }
            let table = step["table"].as_str().unwrap_or("");
            say(&format!("Dropping {table}\u{2026}"));
            let rows = in_transaction(conn, || {
                let rows = count(conn, &format!("SELECT COUNT(*) FROM {table}"), [])?;
                conn.execute_batch(&format!("DROP TABLE IF EXISTS {table}"))?;
                Ok(rows)
            })?;
            removed.0.insert(table.to_owned(), json!(rows));
            dropped.push(table.to_owned());
        }
        Ok(true)
    };
    let finished = steps(&mut removed, &mut dropped)?;

    let counts: Map<String, Value> = removed
        .0
        .into_iter()
        .filter(|(_, n)| crate::pyval::truthy(n))
        .collect();
    let total: i64 = counts.values().filter_map(Value::as_i64).sum();
    let mut entry = json!({"removed": counts, "rows": total, "dropped": dropped});
    if !finished {
        entry["stopped"] = json!(true);
    }
    journal.append(CLEANED, &entry)?;
    let mut out = json!({"removed": counts, "dropped": dropped, "stopped": !finished});
    // VACUUM can't be stopped halfway: the next run compacts
    if !finished || stop() || (!is(&now, "compact") && counts.is_empty() && dropped.is_empty()) {
        out["compacted"] = json!(false);
        return Ok(out);
    }
    let before = store_size(db.path());
    say("Compacting FERB's database (it writes a full copy, so it can take a few minutes)\u{2026}");
    let compaction = db.compact(|folder| (run.free)(folder))?;
    out["compacted"] = json!(compaction.compacted);
    out["needed"] = json!(compaction.needed);
    out["free"] = json!(compaction.free);
    if compaction.compacted {
        let after = store_size(db.path());
        journal.append(
            COMPACTED,
            &json!({"bytes_before": before, "bytes_after": after}),
        )?;
        out["bytes_before"] = json!(before);
        out["bytes_after"] = json!(after);
    }
    Ok(out)
}
