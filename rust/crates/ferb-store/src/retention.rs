//! Age-based retention in one place: what goes when, table by table. A port of
//! `ferb/store/retention.py` (decisions: STORE-5, STORE-400, STORE-401, STORE-404, STORE-406,
//! STORE-407, STORE-408, STORE-420; SENSE-1). Read first (`plan`, any connection), deleted in
//! short batches, dependents first (`delete_rows`). It never deletes what nothing else
//! preserves: the task list and results, episodes, memory, verdicts, schedules, chat and day
//! summaries are named nowhere here.
//!
//! Seam: the settings come in as `get(section, key, default)` (`ferb/config.py`); the daemon
//! decides the cutoffs and runs the pass.

use std::collections::HashMap;

use rusqlite::{named_params, params_from_iter, types::Value as Sql, Connection, ToSql};
use serde_json::{json, Map, Value};

use crate::{pyval::marks, spans::tree_where, Db, Result};

/// A retention of 0 days would purge what was just kept (`retention.py:31`; decision:
/// STORE-420).
pub const MIN_DAYS: i64 = 1;
/// `retention.py:32` (decision: STORE-401).
pub const OPERATIONAL_DAYS: i64 = 90;
/// `retention.py:33` (decision: SENSE-1).
pub const ACCESSIBILITY_DAYS: i64 = 7;
/// `retention.py:34`: typed and copied text expires after 7 days.
pub const TYPED_DAYS: i64 = 7;
/// Rows a delete transaction takes (`retention.py:36`; decision: STORE-406).
pub const BATCH: usize = 200;

/// STORE-401's "operational journal kinds" (`retention.py:40-54`; decision: STORE-408), in
/// sorted order as the query binds them (`retention.py:225`). Owner and chat kinds, day
/// summaries, schedule, timer-set and memory records, and the audit kinds (decision: STORE-418)
/// are not among them and stay.
pub const OPERATIONAL_KINDS: [&str; 51] = [
    "attention.failed",
    "browser.focus_returned",
    "daemon.background_failed",
    "daemon.internal_error",
    "daemon.loop_failed",
    "daemon.slow_call",
    "daemon.started",
    "daemon.stopped",
    "daemon.unhandled",
    "episode.failed",
    "loop.round",
    "mac.back_in_use",
    "memory.cards_failed",
    "memory.consolidate_failed",
    "memory.distill_failed",
    "memory.episode_deferred",
    "memory.episode_failed",
    "memory.episode_refs_damaged",
    "memory.episode_refs_expired",
    "memory.mac_profile_failed",
    "memory.playbooks_failed",
    "memory.profile_failed",
    "memory.search_reindex_failed",
    "plan.extended",
    "plan.repair",
    "plan.step",
    "plan.tidied",
    "preview.failed",
    "preview.slow",
    "resources.failed",
    "retention.downloads",
    "retention.failed",
    "retention.purged",
    "retention.snapshot_purged",
    "runtime.cache_fallback",
    "runtime.cache_skipped",
    "runtime.failed",
    "runtime.stale_runs_swept",
    "screen.capture_stalled",
    "screen.capture_timeout",
    "screen.image_refused",
    "sense.watcher_failed",
    "store.wal_not_truncated",
    "task.dispatch_failed",
    "task.send_digest",
    "task.steering_delivered",
    "tasks.interrupted_after_restart",
    "timer.failed",
    "trail.purged",
    "voice.attached",
    "voice.switched",
];

/// A task id's tree: the part before its first "." (`retention.py:56`).
fn root_of(column: &str) -> String {
    format!(
        "CASE WHEN instr({column}, '.') > 0 THEN substr({column}, 1, instr({column}, '.') - 1) ELSE {column} END"
    )
}

const ENDED: &str = "SELECT id FROM tasks WHERE parent_id IS NULL AND status NOT IN ('running', 'planned', 'paused') \
                     AND updated_at < :cutoff";
const KNOWN: &str = "SELECT id FROM tasks WHERE parent_id IS NULL";
const LIVE: &str =
    "SELECT id FROM tasks WHERE parent_id IS NULL AND (status IN ('running', 'planned', 'paused') \
                    OR updated_at >= :cutoff)";
/// (table, the column naming its task or tree, its own last-write column): records that go with
/// their task (`retention.py:63-64`).
const BY_TASK: [(&str, &str, &str); 4] = [
    ("action_ledger", "task_id", "updated_at"),
    ("confirmations", "tree_id", "at"),
    ("loop_state", "task_id", "updated_at"),
    ("shell_downloads", "tree_id", "at"),
];

/// What goes with a deleted row: (dependent table, its column, the parent's column it names)
/// (`retention.py:66-70`; decision: STORE-406).
pub fn dependents(table: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    match table {
        "task_plans" => &[("plan_steps", "task_id", "task_id")],
        "timers" => &[("timer_deliveries", "timer_id", "id")],
        "trail" => &[("perception_receipts", "event_id", "id")],
        _ => &[],
    }
}

/// `days` (`retention.py:73-75`): a `retention.*_days` setting in whole days, never under
/// `MIN_DAYS` (decision: STORE-420).
pub fn days(value: i64) -> i64 {
    value.max(MIN_DAYS)
}

const DB: &str = "FERB's database on this Mac (~/.ferb/ferb.db)";

/// One row of what FERB keeps and for how long (`retention.py:81-92`): `window` is the pass's
/// name for its days; `setting` the config key (section, key) the owner changes, with
/// `default`; no setting and a `default`: fixed days; neither: kept until `kept` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub records: &'static str,
    pub place: &'static str,
    pub kept: &'static str,
    pub window: Option<&'static str>,
    pub setting: Option<(&'static str, &'static str)>,
    pub default: Option<i64>,
}

const fn rule(
    records: &'static str,
    place: &'static str,
    kept: &'static str,
    window: Option<&'static str>,
    setting: Option<(&'static str, &'static str)>,
    default: Option<i64>,
) -> Rule {
    Rule {
        records,
        place,
        kept,
        window,
        setting,
        default,
    }
}

/// The rule table (`retention.py:98-127`; decision: STORE-5): the hourly pass's windows, the
/// retention settings' defaults and About's "what FERB keeps" rows all read it.
pub const RULES: [Rule; 15] = [
    rule("your tasks, their results and your verdicts", DB, "forever", None, None, None),
    rule("what each task did (episodes) and how-notes", DB, "forever", None, None, None),
    rule(
        "what FERB remembers about you and what it learned",
        DB,
        "until you forget it (Memory page)",
        None,
        None,
        None,
    ),
    rule(
        "your chats with FERB, notices and day summaries",
        DB,
        "forever (a notice's text from typed or copied text: with that text)",
        None,
        None,
        None,
    ),
    rule(
        "schedules, reminders still to come and the sites and people you approved",
        DB,
        "until you remove them (a reminder that went off or was cancelled goes with the operational records)",
        None,
        None,
        None,
    ),
    rule(
        "each task's full trace (every model and tool call)",
        DB,
        "after the task ends, once its episode exists",
        Some("trace_days"),
        Some(("retention", "trace_days")),
        Some(30),
    ),
    rule(
        "operational records (actions taken, confirmations, plans, loop state, finished reminders and timers, FERB's \
         own bookkeeping)",
        DB,
        "after their task or timer ends",
        Some("operational_days"),
        Some(("retention", "operational_days")),
        Some(OPERATIONAL_DAYS),
    ),
    rule(
        "Jev's decisions (Jev is switched off)",
        DB,
        "after they were made",
        Some("decision_days"),
        Some(("retention", "decision_days")),
        Some(30),
    ),
    rule(
        "text read from apps on screen (accessibility)",
        DB,
        "after it was read",
        None,
        None,
        Some(ACCESSIBILITY_DAYS),
    ),
    rule(
        "text you typed or copied, and notice text that came from it",
        DB,
        "after it was typed or copied (the record that you typed or copied stays)",
        None,
        None,
        Some(TYPED_DAYS),
    ),
    rule(
        "other activity on this Mac (apps, windows, pages, files, notifications)",
        DB,
        "once that day's summary exists",
        Some("trail_days"),
        Some(("sense", "trail_retention_days")),
        Some(30),
    ),
    rule(
        "screenshots, evidence and shell output",
        "~/.ferb/evidence and ~/.ferb/logs/shell on this Mac",
        "after they were captured",
        Some("evidence_days"),
        Some(("retention", "evidence_days")),
        Some(7),
    ),
    rule(
        "copies of files before FERB changed them, records of what it moved to the Trash, disk snapshots",
        "~/.ferb/recovery on this Mac (snapshots: your disk)",
        "after FERB made them",
        Some("recovery_days"),
        Some(("retention", "recovery_days")),
        Some(7),
    ),
    rule(
        "FERB's own CPU and memory use",
        DB,
        "after it was measured",
        Some("resource_days"),
        Some(("retention", "resource_days")),
        Some(30),
    ),
    rule(
        "secrets and card details you saved",
        "your login Keychain (never the database)",
        "until you delete them (Settings \u{2192} Data)",
        None,
        None,
        None,
    ),
];

/// `windows` (`retention.py:130-133`): the pass's windows in days from config
/// (`get(section, key, default)`), each at least `MIN_DAYS`, in the table's order.
pub fn windows(get: &dyn Fn(&str, &str, i64) -> i64) -> Vec<(&'static str, i64)> {
    RULES
        .iter()
        .filter_map(|r| match (r.window, r.setting, r.default) {
            (Some(window), Some((section, key)), Some(default)) => {
                Some((window, days(get(section, key, default))))
            }
            _ => None,
        })
        .collect()
}

/// `describe` (`retention.py:136-146`): the rule table as the owner reads it: what, where, for
/// how long (days from config), and the setting that changes it.
pub fn describe(get: &dyn Fn(&str, &str, i64) -> i64) -> Vec<Value> {
    RULES
        .iter()
        .map(|r| {
            let n = match (r.setting, r.default) {
                (Some((section, key)), Some(default)) => Some(days(get(section, key, default))),
                (_, default) => default,
            };
            let mut out = Map::new();
            out.insert("records".to_owned(), json!(r.records));
            out.insert("where".to_owned(), json!(r.place));
            out.insert(
                "kept".to_owned(),
                match n {
                    Some(n) => json!(format!(
                        "{n} day{} {}",
                        if n != 1 { "s" } else { "" },
                        r.kept
                    )),
                    None => json!(r.kept),
                },
            );
            if let Some(n) = n {
                out.insert("days".to_owned(), json!(n));
            }
            if let Some((section, key)) = r.setting {
                out.insert("setting".to_owned(), json!(format!("{section}.{key}")));
            }
            Value::Object(out)
        })
        .collect()
}

/// What one pass deletes, read before anything is deleted (`retention.py:149-158`): each old
/// tree's spans (children before parents), loose spans, Jev decisions and operational rows by
/// table (rowids). `left`: old trees waiting for the next pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub roots: Vec<String>,
    /// By root, in `roots`' order.
    pub trees: Vec<(String, Vec<String>)>,
    pub loose: Vec<String>,
    pub decisions: Vec<i64>,
    pub operational: Vec<(&'static str, Vec<i64>)>,
    pub left: usize,
}

fn column<T: rusqlite::types::FromSql>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params, |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `plan` (`retention.py:161-187`; decisions: STORE-400, STORE-406): read what a pass deletes
/// (any connection; the daemon reads it off its loop). At most `roots_limit` old trees, oldest
/// first; the rest are counted in `left`.
pub fn plan(
    conn: &Connection,
    trace_cutoff: &str,
    decision_cutoff: &str,
    operational_cutoff: &str,
    roots_limit: usize,
) -> Result<Plan> {
    let mut roots = finished_roots_before(conn, trace_cutoff)?;
    let left = roots.len().saturating_sub(roots_limit);
    roots.truncate(roots_limit);
    let mut out = Plan {
        roots,
        left,
        ..Plan::default()
    };
    let mut requests: HashMap<String, Option<String>> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT id, request_id FROM tasks WHERE id IN ({})",
            marks(out.roots.len())
        ))?;
        let mut rows = statement.query(params_from_iter(out.roots.iter()))?;
        while let Some(row) = rows.next()? {
            requests.insert(row.get(0)?, row.get(1)?);
        }
    }
    // the intake planner's spans carry the request id, not a task id: one scan for every tree
    let mut intake: HashMap<String, Vec<String>> = HashMap::new();
    if !requests.is_empty() {
        let mut statement = conn.prepare(
            "SELECT id, json_extract(input_json, '$.request_id') FROM spans WHERE task_id IS NULL",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            // only a text request id can be a task's (tasks.request_id is text)
            if let Sql::Text(request) = row.get::<_, Sql>(1)? {
                if requests.values().any(|r| r.as_deref() == Some(&request)) {
                    intake.entry(request).or_default().push(row.get(0)?);
                }
            }
        }
    }
    for root in &out.roots {
        let (tree, p) = tree_where(root);
        let mut seeds: Vec<String> = column(
            conn,
            &format!("SELECT id FROM spans WHERE {tree}"),
            params_from_iter(p.iter()),
        )?;
        let request = requests.get(root).cloned().flatten().unwrap_or_default();
        seeds.extend(intake.get(&request).cloned().unwrap_or_default());
        out.trees
            .push((root.clone(), with_descendants(conn, seeds)?));
    }
    let loose: Vec<String> = column(
        conn,
        "SELECT id FROM spans WHERE task_id IS NULL AND parent_id IS NULL AND started_at < ? AND \
         (json_extract(input_json, '$.request_id') IS NULL OR \
          json_extract(input_json, '$.request_id') NOT IN (SELECT request_id FROM tasks))",
        [trace_cutoff],
    )?;
    out.loose = with_descendants(conn, loose)?;
    out.decisions = column(
        conn,
        "SELECT rowid FROM decisions WHERE at < ?",
        [decision_cutoff],
    )?;
    out.operational = operational(conn, operational_cutoff)?;
    Ok(out)
}

/// `_with_descendants` (`retention.py:190-204`): these spans and every span under them, deepest
/// first: a parent is never deleted before its child (spans.parent_id is a foreign key). Like
/// Python's, it does not end on a parent cycle (which the foreign key alone does not forbid).
fn with_descendants(conn: &Connection, seeds: Vec<String>) -> Result<Vec<String>> {
    // Python's dict: first-seen order kept, a depth updated in place.
    let mut order: Vec<String> = Vec::new();
    let mut depth: HashMap<String, i64> = HashMap::new();
    for seed in seeds {
        if let std::collections::hash_map::Entry::Vacant(slot) = depth.entry(seed.clone()) {
            slot.insert(0);
            order.push(seed);
        }
    }
    let mut frontier = order.clone();
    let mut level = 0i64;
    while !frontier.is_empty() {
        level += 1;
        let mut children: Vec<String> = Vec::new();
        for chunk in frontier.chunks(500) {
            children.extend(column::<String>(
                conn,
                &format!(
                    "SELECT id FROM spans WHERE parent_id IN ({})",
                    marks(chunk.len())
                ),
                params_from_iter(chunk),
            )?);
        }
        // walked again when found deeper
        frontier = children
            .into_iter()
            .filter(|c| depth.get(c).copied().unwrap_or(-1) < level)
            .collect();
        for child in &frontier {
            if depth.insert(child.clone(), level).is_none() {
                order.push(child.clone());
            }
        }
    }
    order.sort_by_key(|ident| -depth[ident]); // stable, as Python's sorted
    Ok(order)
}

/// `operational` (`retention.py:207-231`; decisions: STORE-401, STORE-404, STORE-407,
/// STORE-408): the rowids of operational records whose task or timer ended before `cutoff`, by
/// table; a table with none is left out.
pub fn operational(conn: &Connection, cutoff: &str) -> Result<Vec<(&'static str, Vec<i64>)>> {
    let args = named_params! {":cutoff": cutoff};
    let mut found: Vec<(&'static str, Vec<i64>)> = Vec::new();
    for (table, task_column, written) in BY_TASK {
        let root = root_of(task_column);
        found.push((
            table,
            column(
                conn,
                &format!(
                    "SELECT rowid FROM {table} WHERE {root} IN ({ENDED}) OR ({root} NOT IN ({KNOWN}) AND {written} < :cutoff)"
                ),
                args,
            )?,
        ));
    }
    found.push((
        "task_plans",
        column(
            conn,
            &format!(
                "SELECT rowid FROM task_plans WHERE {} IN ({ENDED}) \
                 AND status NOT IN ('running', 'suspended', 'steps_done')",
                root_of("task_id")
            ),
            args,
        )?,
    ));
    found.push((
        "timers",
        column(
            conn,
            "SELECT rowid FROM timers WHERE state IN ('delivered', 'cancelled') AND COALESCE(delivered_at, fire_at) < :cutoff",
            args,
        )?,
    ));
    found.push((
        "timer_deliveries", // one left by a timer already gone
        column(
            conn,
            "SELECT rowid FROM timer_deliveries WHERE at < :cutoff AND timer_id NOT IN (SELECT id FROM timers)",
            args,
        )?,
    ));
    let task = root_of("json_extract(data_json, '$.task_id')");
    let names: Vec<String> = (0..OPERATIONAL_KINDS.len())
        .map(|i| format!(":k{i}"))
        .collect();
    let mut bound: Vec<(&str, &dyn ToSql)> = vec![(":cutoff", &cutoff)];
    for (name, kind) in names.iter().zip(OPERATIONAL_KINDS.iter()) {
        bound.push((name.as_str(), kind));
    }
    found.push((
        "journal",
        column(
            conn,
            &format!(
                "SELECT rowid FROM journal WHERE kind IN ({}) \
                 AND julianday(at) < julianday(:cutoff) \
                 AND (json_extract(data_json, '$.task_id') IS NULL OR {task} NOT IN ({LIVE})) \
                 AND id NOT IN (SELECT source_id FROM memory_sources WHERE kind = 'journal')",
                names.join(",")
            ),
            bound.as_slice(),
        )?,
    ));
    found.retain(|(_, ids)| !ids.is_empty());
    Ok(found)
}

/// `finished_roots_before` (`retention.py:234-241`): root tasks that ended before the cutoff
/// and still have spans, oldest first (never a running, planned or paused one).
pub fn finished_roots_before(conn: &Connection, cutoff: &str) -> Result<Vec<String>> {
    column(
        conn,
        "SELECT id FROM tasks t WHERE parent_id IS NULL AND status NOT IN ('running', 'planned', 'paused') AND updated_at < ? \
         AND EXISTS (SELECT 1 FROM spans WHERE task_id = t.id OR (task_id >= t.id || '.' AND task_id < t.id || '/')) \
         ORDER BY updated_at, rowid",
        [cutoff],
    )
}

/// `delete_rows` (`retention.py:244-262`; decision: STORE-406): delete `table`'s rows whose
/// `key` is in `ids`, in the order given, `batch` rows a transaction, each batch's dependents
/// in the same transaction first. An iterator of each batch's count, so the daemon's loop runs
/// between batches; nothing is deleted until it is advanced. A failed batch is rolled back
/// whole and returned as the error. Table and column names are FERB's own constants.
pub fn delete_rows<'a>(
    db: &'a Db,
    table: &'a str,
    key: &'a str,
    ids: &'a [Sql],
    batch: usize,
) -> impl Iterator<Item = Result<usize>> + 'a {
    assert!(batch > 0, "a batch holds at least one row");
    ids.chunks(batch).map(move |chunk| {
        let conn = db.conn();
        let marks = marks(chunk.len());
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let run = || -> Result<usize> {
            for (dependent, column, parent) in dependents(table) {
                conn.execute(
                    &format!(
                        "DELETE FROM {dependent} WHERE {column} IN \
                         (SELECT {parent} FROM {table} WHERE {key} IN ({marks}))"
                    ),
                    params_from_iter(chunk),
                )?;
            }
            let n = conn.execute(
                &format!("DELETE FROM {table} WHERE {key} IN ({marks})"),
                params_from_iter(chunk),
            )?;
            conn.execute_batch("COMMIT")?;
            Ok(n)
        };
        let done = run();
        if done.is_err() && !conn.is_autocommit() {
            // SQLite may have rolled back already; keep the real error
            let _ = conn.execute_batch("ROLLBACK");
        }
        done
    })
}
