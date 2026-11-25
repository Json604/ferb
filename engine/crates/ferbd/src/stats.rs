//! `ferb stats` (`ferb/cli/admin.py` `stats` with `_repeats`, `_daemon_problems`, `shown_bytes`, `_share`,
//! `_resources`, `_window` and `status_label`): how FERB is really doing, read straight from the database,
//! read-only, so it works whether or not the daemon runs. Task outcomes, the top tool-error reasons, sandbox timing,
//! confirmations, containment, model and learning runs, the daemon's own failures and slow answers, repeated work
//! (playbooks) and FERB's own resource use per local day (L30). Every line is written as Python's `print` wrote it.
//!
//! Time comes in as [`Now`] (Python's `datetime.now(timezone.utc)` for the sections counted back `days` × 24 h, and
//! `datetime.now().astimezone().date()` for the resource log's whole local days).
//!
//! Where Python raised (a table missing, a stored value of a shape FERB never writes), the command stops there as
//! Python's uncaught exception did: what was printed stays, one line goes to stderr (Python printed a traceback) and
//! the exit status is 1.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use ferb_py::time::{DateTime, Timespec};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value};

/// The instant `ferb stats` runs at: UTC, and the local wall time of the same instant (naive).
#[derive(Clone, Copy, Debug)]
pub struct Now {
    /// `datetime.now(timezone.utc)`: aware, offset 0.
    pub utc: DateTime,
    /// `datetime.now()`: this device's local wall time; only its date is read.
    pub local: DateTime,
}

impl Now {
    /// The real clock.
    pub fn real() -> Now {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Now {
            utc: DateTime::utc_from_unix(since.as_secs() as i64, since.subsec_micros()),
            local: DateTime::now_local(),
        }
    }
}

type Res<T> = Result<T, String>;

/// `ferb stats --days N [--from DAY --to DAY]` on the real clock.
pub fn stats_now(
    home: &Path,
    out: &mut dyn Write,
    days: i64,
    range: Option<(String, String)>,
) -> i32 {
    stats_in(home, out, days, range, Now::real())
}

/// `admin.stats(home, out, days)`: 0, or 1 with "no database at …" when there is none (or when Python would have
/// raised: the reason goes to stderr).
pub fn stats(home: &Path, out: &mut dyn Write, days: i64, now: Now) -> i32 {
    stats_in(home, out, days, None, now)
}

/// [`stats`], the model-use section over local days `range` (both included) instead of the last `days` days
/// (ORCH-720). The sections Python printed keep `days`.
pub fn stats_in(
    home: &Path,
    out: &mut dyn Write,
    days: i64,
    range: Option<(String, String)>,
    now: Now,
) -> i32 {
    // pathlib's Path(str(home)) / "ferb.db": repeated and trailing separators and inner "." dropped
    let db: PathBuf = home.components().collect::<PathBuf>().join("ferb.db");
    if !db.exists() {
        let _ = writeln!(out, "no database at {}", db.display());
        return 1;
    }
    match run(&db, out, days, range, &now) {
        Ok(()) => 0,
        Err(e) => {
            let _ = out.flush();
            eprintln!("ferb stats: {e}");
            1
        }
    }
}

fn say(out: &mut dyn Write, line: &str) -> Res<()> {
    writeln!(out, "{line}").map_err(|e| format!("BrokenPipeError: {e}"))
}

fn sq(e: rusqlite::Error) -> String {
    format!("sqlite3.OperationalError: {e}")
}

fn run(
    db: &Path,
    out: &mut dyn Write,
    days: i64,
    range: Option<(String, String)>,
    now: &Now,
) -> Res<()> {
    let since = days
        .checked_mul(-86_400)
        .and_then(|s| now.utc.plus(s, 0).ok())
        .ok_or("OverflowError: date value out of range")?
        .isoformat(Timespec::Auto);
    let path = db.to_str().ok_or("UnicodeEncodeError: the database path")?;
    // sqlite3.connect(f"file:{db}?mode=ro", uri=True): the same URI, read-only, the default 5 s busy wait
    let c = Connection::open_with_flags(
        format!("file:{path}?mode=ro"),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sq)?;
    c.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(sq)?;
    let s = Sql::Text(since.clone());

    let outcomes = rows(
        &c,
        "SELECT status, COUNT(*) FROM tasks WHERE parent_id IS NULL AND created_at >= ? \
         GROUP BY status ORDER BY 2 DESC",
        std::slice::from_ref(&s),
    )?;
    let total: i64 = outcomes.iter().map(|r| int(&r[1])).sum();
    let count = |status: &str| -> i64 {
        outcomes
            .iter()
            .rev()
            .find(|r| matches!(&r[0], Sql::Text(t) if t == status))
            .map_or(0, |r| int(&r[1]))
    };
    // V4: done means every condition verified; not confirmed (done_unverified) means done but it stands
    // unconfirmed; failed, not done. V5: the label never says "unverified".
    let mut head = format!("Requests in the last {days} day(s): {total}");
    if total != 0 {
        let shares: Vec<String> = [
            ("done", "done"),
            ("not confirmed", "done_unverified"),
            ("with gaps", "done_with_gaps"),
            ("failed", "failed"),
        ]
        .iter()
        // ORCH-651: "with gaps" only when there are some, so a store without one reads as before
        .filter(|(_, st)| *st != "done_with_gaps" || count(st) > 0)
        .map(|(label, st)| {
            let n = count(st);
            format!("{label} {n} ({}%)", (100 * n).div_euclid(total))
        })
        .collect();
        head.push_str(&format!(" — {}", shares.join(", ")));
    }
    say(out, &head)?;
    for r in &outcomes {
        say(
            out,
            &format!("  {}{}", ljust(&status_label(&r[0]), 16), sql_str(&r[1])),
        )?;
    }

    let r = one(
        &c,
        "SELECT COUNT(*), SUM(error IS NOT NULL) FROM spans WHERE kind = 'tool.call' AND started_at >= ?",
        std::slice::from_ref(&s),
    )?;
    say(
        out,
        &format!("Tool calls: {}, errored: {}", or0(&r[0]), or0(&r[1])),
    )?;
    for r in rows(
        &c,
        "SELECT substr(error, 1, 90), COUNT(*) FROM spans WHERE kind = 'tool.call' AND \
         error IS NOT NULL AND started_at >= ? GROUP BY 1 ORDER BY 2 DESC LIMIT 10",
        std::slice::from_ref(&s),
    )? {
        say(
            out,
            &format!("  {}  {}", rjust(&sql_str(&r[1]), 4), sql_str(&r[0])),
        )?;
    }

    let mut values = Vec::new();
    for r in rows(
        &c,
        "SELECT json_extract(output_json, '$.sandbox_ms') FROM spans WHERE kind='tool.call' AND started_at >= ? \
         AND json_extract(output_json, '$.sandbox_ms') IS NOT NULL",
        std::slice::from_ref(&s),
    )? {
        values.push(py_float(&r[0])?);
    }
    if !values.is_empty() {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = values.len();
        let median = if n % 2 == 1 {
            values[n / 2]
        } else {
            (values[n / 2 - 1] + values[n / 2]) / 2.0
        };
        let at = ((n as f64 * 0.95).ceil() as i64 - 1).max(0) as usize;
        say(
            out,
            &format!(
                "Contained shell start to first output/exit: n={n}, median={} ms, p95={} ms \
                 (includes command startup; not an isolated sandbox overhead)",
                f1(median),
                f1(values[at])
            ),
        )?;
    }

    let prompts = one(
        &c,
        "SELECT COUNT(*) FROM spans WHERE kind='tool.call' AND started_at >= ? \
         AND json_extract(input_json, '$.ferb_question') = 1",
        std::slice::from_ref(&s),
    )?;
    say(
        out,
        &format!("Code confirmation prompts: {}", sql_str(&prompts[0])),
    )?;

    let checks = rows(
        &c,
        "SELECT j.value, COUNT(*) FROM spans, json_each(spans.input_json, '$.point_checks') \
         AS j WHERE spans.kind = 'tool.call' AND spans.started_at >= ? GROUP BY j.value",
        std::slice::from_ref(&s),
    )?;
    if !checks.is_empty() {
        // which check decided each checked click (D31 fix round 4); only a text value can be one of its keys
        let mut counts = Map::new();
        for r in &checks {
            if let Sql::Text(k) = &r[0] {
                counts.insert(k.clone(), Value::from(int(&r[1])));
            }
        }
        say(
            out,
            &ferb_trace::lines::point_checks(&Value::Object(counts)),
        )?;
    }

    let tables: HashSet<String> =
        rows(&c, "SELECT name FROM sqlite_master WHERE type='table'", &[])?
            .into_iter()
            .map(|r| sql_str(&r[0]))
            .collect();
    if tables.contains("confirmations") {
        let (mut yes, mut no) = (0, 0);
        for r in rows(
            &c,
            "SELECT granted, COUNT(*) FROM confirmations WHERE at >= ? GROUP BY granted",
            std::slice::from_ref(&s),
        )? {
            let granted = number(&r[0]);
            if granted == Some(1.0) {
                yes = int(&r[1]);
            } else if granted == Some(0.0) {
                no = int(&r[1]);
            }
        }
        say(out, &format!("Confirmation answers: Yes {yes}, No {no}"))?;
    }

    // the latest network record of each process, keyed as Python's dict keys them
    let mut latest = Ordered::default();
    let mut shell_calls = 0;
    for r in rows(
        &c,
        "SELECT input_json, output_json FROM spans WHERE kind='tool.call' AND started_at >= ? ORDER BY rowid",
        std::slice::from_ref(&s),
    )? {
        let inp = loads_or_empty(&r[0])?;
        let result = loads_or_empty(&r[1])?;
        let (inp, result) = (object(&inp)?, object(&result)?);
        if matches!(inp.get("tool"), Some(Value::String(t)) if t.starts_with("shell.")) {
            shell_calls += 1;
        }
        if let (Some(id), Some(net @ Value::Object(_))) = (result.get("id"), result.get("network")) {
            if ferb_py::value::truthy(id) {
                latest.set(hash_key(id)?, net.clone());
            }
        }
    }
    let mut destinations: HashSet<(String, String)> = HashSet::new();
    let mut refused = Num::I(0);
    for net in latest.values() {
        for d in iterate(net.get("destinations"))? {
            let d = d
                .as_object()
                .ok_or("TypeError: string indices must be integers")?;
            let host = d.get("host").ok_or("KeyError: 'host'")?;
            let port = d.get("port").ok_or("KeyError: 'port'")?;
            destinations.insert((hash_key(host)?, hash_key(port)?));
        }
        for r in iterate(net.get("refused"))? {
            let r = r
                .as_object()
                .ok_or("AttributeError: object has no attribute 'get'")?;
            refused.add(r.get("count").unwrap_or(&Value::from(0)))?;
        }
    }
    let escapes = one(
        &c,
        "SELECT COUNT(*) FROM journal WHERE kind='escape.used' AND at >= ?",
        std::slice::from_ref(&s),
    )?;
    say(
        out,
        &format!(
            "Containment: shell calls {shell_calls}, observed destinations {}, refused connections {}, \
             escapes used {}",
            destinations.len(),
            refused.shown(),
            sql_str(&escapes[0])
        ),
    )?;

    for r in rows(
        &c,
        "SELECT kind, COUNT(*), ROUND(AVG((julianday(ended_at) - julianday(started_at)) * 86400), 1) \
         FROM spans WHERE kind IN ('model.turn', 'worker') AND started_at >= ? GROUP BY kind",
        std::slice::from_ref(&s),
    )? {
        say(
            out,
            &format!(
                "Model runs ({}): {}, avg {} s",
                sql_str(&r[0]),
                sql_str(&r[1]),
                or0(&r[2])
            ),
        )?;
    }

    // each learning model call is one model.learning span (memory/calls.py): consolidation and distillation
    let r = one(
        &c,
        "SELECT COUNT(*), SUM(error IS NOT NULL) FROM spans WHERE kind = 'model.learning' AND started_at >= ?",
        std::slice::from_ref(&s),
    )?;
    say(
        out,
        &format!(
            "Learning model calls: {}, failed: {}",
            or0(&r[0]),
            or0(&r[1])
        ),
    )?;
    // grouped with their numbers set aside (a count Codex measured differs per call); the latest wording shown
    let mut groups: Vec<((String, String), i64, String)> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for r in rows(
        &c,
        "SELECT json_extract(input_json, '$.role'), error FROM spans WHERE \
         kind = 'model.learning' AND error IS NOT NULL AND started_at >= ? ORDER BY started_at, rowid",
        std::slice::from_ref(&s),
    )? {
        let error = sql_str(&r[1]);
        let key = (sql_str(&r[0]), numbers_set_aside(&error));
        let i = *at.entry(key.clone()).or_insert_with(|| {
            groups.push((key, 0, String::new()));
            groups.len() - 1
        });
        groups[i].1 += 1;
        groups[i].2 = error;
    }
    groups.sort_by_key(|g| -g.1);
    for ((role, _), n, latest) in groups.iter().take(10) {
        say(
            out,
            &format!(
                "  {}  {role}: {}",
                rjust(&n.to_string(), 4),
                ferb_py::value::prefix(latest, 160)
            ),
        )?;
    }
    let last = rows(
        &c,
        "SELECT json_extract(input_json, '$.call') FROM spans WHERE kind = 'model.learning' AND \
         error IS NOT NULL AND started_at >= ? ORDER BY started_at DESC, rowid DESC LIMIT 1",
        std::slice::from_ref(&s),
    )?;
    if let Some(r) = last.first() {
        if sql_truthy(&r[0]) {
            say(
                out,
                &format!("  latest failed call: ferb trace {}", sql_str(&r[0])),
            )?;
        }
    }

    daemon_problems(&c, &s, out)?;
    if tables.contains("learning_nodes") && tables.contains("episodes") {
        repeats(&c, out)?;
    }
    resources(&c, days, tables.contains("resource_samples"), now, out)?;
    // ORCH-720: what each role's model calls cost (printed only when usage was recorded in the range)
    let range = match range {
        Some((from, to)) => crate::usage_stats::Range::Days { from, to },
        None => crate::usage_stats::Range::Since { since, days },
    };
    crate::usage_stats::report(&c, &range, out)
}

/// `status_label`: a task status as the owner reads it: done_unverified is "not confirmed" (V5); the others as
/// stored.
pub fn status_label(status: &Sql) -> String {
    match status {
        Sql::Text(t) if t == "done_unverified" => "not confirmed".to_owned(),
        Sql::Text(t) if t == "done_with_gaps" => "with gaps".to_owned(), // ORCH-651
        other => sql_str(other),
    }
}

/// `_daemon_problems`: requests that failed inside FERB (the server's catch-all) and those answered after
/// `CLIENT_WAIT_S`, as the daemon journaled them (L36), a row with `repeats` standing for that many.
fn daemon_problems(c: &Connection, since: &Sql, out: &mut dyn Write) -> Res<()> {
    use ferb_daemon::areas_b::reports::{INTERNAL_ERROR, SLOW_CALL};
    let journaled = |kind: &str| -> Res<Vec<Map<String, Value>>> {
        let mut found = Vec::new();
        for r in rows(
            c,
            "SELECT data_json FROM journal WHERE kind = ? AND at >= ? ORDER BY seq",
            &[Sql::Text(kind.to_owned()), since.clone()],
        )? {
            let v = loads(&r[0])?;
            found.push(object(&v)?.clone());
        }
        Ok(found)
    };
    let failed = journaled(INTERNAL_ERROR)?;
    let mut total: i128 = 0;
    for d in &failed {
        total += times(d)?;
    }
    say(out, &format!("Requests that failed inside FERB: {total}"))?;
    let mut groups: Vec<(Key, i128, &Map<String, Value>)> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for d in &failed {
        let key = (get_str(d, "method"), get_str(d, "error"));
        let i = *at.entry(key.clone()).or_insert_with(|| {
            groups.push((key, 0, d));
            groups.len() - 1
        });
        groups[i].1 += times(d)?;
        groups[i].2 = d;
    }
    groups.sort_by_key(|g| -g.1);
    for ((method, error), n, d) in groups.iter().take(10) {
        let message = d.get("message").map_or_else(String::new, pystr);
        say(
            out,
            &format!(
                "  {}  {method}: {error}: {}",
                rjust(&n.to_string(), 4),
                ferb_py::value::prefix(&message, 160)
            ),
        )?;
        let frames = match d.get("frames") {
            Some(v) if ferb_py::value::truthy(v) => listed(v)?,
            _ => Vec::new(),
        };
        for frame in &frames[frames.len().saturating_sub(3)..] {
            say(out, &format!("          at {}", pystr(frame)))?;
        }
    }
    let slow = journaled(SLOW_CALL)?;
    let mut total: i128 = 0;
    for d in &slow {
        total += times(d)?;
    }
    say(
        out,
        &format!(
            "Requests answered after {} s: {total}",
            ferb_clock::py::fmt_g(ferb_rpc::server::CLIENT_WAIT_S)
        ),
    )?;
    for d in &slow[slow.len().saturating_sub(10)..] {
        let more = match d.get("repeats") {
            Some(v) if ferb_py::value::truthy(v) => {
                format!(" ({} times in its window; this is the latest)", pystr(v))
            }
            _ => String::new(),
        };
        say(
            out,
            &format!(
                "        {}: {} s{more}",
                get_str(d, "method"),
                get_str(d, "seconds")
            ),
        )?;
    }
    Ok(())
}

/// (method, error) or (role, error with its numbers set aside).
type Key = (String, String);

/// `times(d)`: `int(d.get("repeats") or 1)`: a window's row stands for its repeats, any other for one.
fn times(d: &Map<String, Value>) -> Res<i128> {
    match d.get("repeats") {
        Some(v) if ferb_py::value::truthy(v) => py_int(v),
        _ => Ok(1),
    }
}

/// `_repeats`: "gets better at repeated tasks", measured (kb area memory): each playbook's verified runs of one
/// kind of work, first and latest, with attempts and time, and how many later runs were shown its memory.
fn repeats(c: &Connection, out: &mut dyn Write) -> Res<()> {
    let books = repeat_stats(c)?;
    say(out, &format!("Repeated work (playbooks): {}", books.len()))?;
    for book in books.iter().take(10) {
        let runs = &book.runs;
        let (Some(first), Some(last)) = (runs.first(), runs.last()) else {
            continue;
        };
        let used: Vec<bool> = runs[1..].iter().filter_map(|r| r.used_memory).collect();
        say(
            out,
            &format!(
                "  {}: {} runs; first {} attempt(s), {} s; latest {} attempt(s), {} s; memory shown in {} of \
                 {} later traced run(s)",
                book.title,
                runs.len(),
                first.attempts,
                ferb_clock::py::fmt_g(first.seconds),
                last.attempts,
                ferb_clock::py::fmt_g(last.seconds),
                used.iter().filter(|u| **u).count(),
                used.len()
            ),
        )?;
    }
    Ok(())
}

struct Book {
    /// `str(body.get("title"))`
    title: String,
    runs: Vec<Run>,
}

struct Run {
    task: String,
    started_at: String,
    attempts: i64,
    seconds: f64,
    used_memory: Option<bool>,
}

/// `ferb/memory/playbooks.py` `repeat_stats` on a plain read-only connection, as `ferb stats` called it (the port
/// in `ferb_learning::playbooks::repeat_stats` takes the daemon's writable store): each current playbook's runs
/// oldest first, and whether each later run was shown the playbook or an earlier run's how-note.
fn repeat_stats(c: &Connection) -> Res<Vec<Book>> {
    let mut books = Vec::new();
    for r in rows(
        c,
        "SELECT id, body_json FROM learning_nodes WHERE kind = 'playbook' AND valid_to IS \
         NULL AND state NOT IN ('retired','rolled_back') ORDER BY recorded_at",
        &[],
    )? {
        let nid = r[0].clone();
        let body = loads(&r[1])?;
        let body = object(&body)?;
        let tasks: Vec<String> = match body.get("tasks") {
            Some(v) if ferb_py::value::truthy(v) => listed(v)?.iter().map(pystr).collect(),
            _ => Vec::new(),
        };
        let mut runs = Vec::new();
        for task in tasks {
            let Some(row) = rows(
                c,
                "SELECT started_at, seconds, steps_json, verified FROM episodes WHERE task_id = ?",
                &[Sql::Text(task.clone())],
            )?
            .into_iter()
            .next() else {
                continue;
            };
            let steps = if sql_truthy(&row[2]) {
                loads(&row[2])?
            } else {
                Value::Array(Vec::new())
            };
            let seconds = if sql_truthy(&row[1]) {
                py_float(&row[1])?
            } else {
                0.0
            };
            runs.push(Run {
                task,
                started_at: if sql_truthy(&row[0]) {
                    sql_str(&row[0])
                } else {
                    String::new()
                },
                attempts: ferb_learning::playbooks::attempts(&steps),
                seconds: ferb_memory::pymath::round(seconds, 1),
                used_memory: None,
            });
        }
        runs.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        let mut refs = vec![nid.clone()];
        for (n, run) in runs.iter_mut().enumerate() {
            let (wh, params) = ferb_store::spans::tree_where(&run.task);
            let p: Vec<Sql> = params.iter().map(|x| Sql::Text(x.clone())).collect();
            let traced =
                !rows(c, &format!("SELECT 1 FROM spans WHERE {wh} LIMIT 1"), &p)?.is_empty();
            run.used_memory = if !traced || n == 0 {
                None
            } else {
                let mut any = false;
                for reference in &refs {
                    let mut q = p.clone();
                    q.push(reference.clone());
                    if !rows(
                        c,
                        &format!(
                            "SELECT 1 FROM spans WHERE {wh} AND kind IN ('context.exposure','context.source') \
                             AND instr(input_json, ?) > 0 LIMIT 1"
                        ),
                        &q,
                    )?
                    .is_empty()
                    {
                        any = true;
                        break;
                    }
                }
                Some(any)
            };
            for e in rows(
                c,
                "SELECT id FROM learning_nodes WHERE kind = 'episode' AND key = ?",
                &[Sql::Text(run.task.clone())],
            )? {
                refs.push(Sql::Text(sql_str(&e[0])));
            }
        }
        books.push(Book {
            title: body.get("title").map_or_else(|| "None".to_owned(), pystr),
            runs,
        });
    }
    Ok(books)
}

/// `shown_bytes`: a size as the owner reads it (decimal units, as Finder shows them).
pub fn shown_bytes(n: f64) -> String {
    if n >= 1e9 {
        format!("{} GB", f1(n / 1e9))
    } else if n >= 1e6 {
        format!("{} MB", f1(n / 1e6))
    } else {
        format!("{} KB", fixed(n / 1e3, 0))
    }
}

/// `_share`: CPU seconds over a time, as a share of one core (Activity Monitor's % CPU).
fn share(cpu: f64, seconds: f64) -> String {
    if seconds > 0.0 {
        format!("{}%", f1(100.0 * cpu / seconds))
    } else {
        "—".to_owned()
    }
}

struct Sample {
    at: String,
    day: String,
    window_s: f64,
    cpu_s: f64,
    footprint: Option<Sql>,
    peak_footprint: Option<Sql>,
    child_cpu_s: f64,
    app_cpu_s: Option<f64>,
    db_bytes: Option<f64>,
    work_json: Sql,
}

/// `_resources`: FERB's resource log (L30) for the last `days` local days, today included, one line per day, then
/// the period's five windows with the most CPU, with what FERB was doing in each. `present`: the store has the
/// log's table.
fn resources(c: &Connection, days: i64, present: bool, now: &Now, out: &mut dyn Write) -> Res<()> {
    let days = days.max(1);
    let period = format!("the last {days} local day(s), today included");
    let local = DateTime {
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset: None,
        ..now.local
    };
    let back = local
        .plus(-(days - 1) * 86_400, 0)
        .map_err(|_| "OverflowError: date value out of range")?;
    let cutoff = format!("{:04}-{:02}-{:02}", back.year, back.month, back.day);
    let mut samples = Vec::new();
    if present {
        for r in rows(
            c,
            "SELECT at, day, window_s, cpu_s, footprint, peak_footprint, child_cpu_s, app_cpu_s, \
             db_bytes, work_json FROM resource_samples WHERE day >= ? ORDER BY day, id",
            &[Sql::Text(cutoff)],
        )? {
            let opt = |v: &Sql| (!matches!(v, Sql::Null)).then(|| v.clone());
            samples.push(Sample {
                at: sql_str(&r[0]),
                day: sql_str(&r[1]),
                window_s: num_req(&r[2])?,
                cpu_s: num_req(&r[3])?,
                footprint: opt(&r[4]),
                peak_footprint: opt(&r[5]),
                child_cpu_s: num_req(&r[6])?,
                app_cpu_s: opt(&r[7]).map(|v| num_req(&v)).transpose()?,
                db_bytes: opt(&r[8]).map(|v| num_req(&v)).transpose()?,
                work_json: r[9].clone(),
            });
        }
    }
    if samples.is_empty() {
        let any = present && !rows(c, "SELECT 1 FROM resource_samples LIMIT 1", &[])?.is_empty();
        let line = if any {
            format!("Resources: no samples in {period}.")
        } else if present {
            "Resources: no samples yet (the daemon writes one every 5 minutes).".to_owned()
        } else {
            "Resources: no samples yet; the resource log starts after the next `ferb setup --update`.".to_owned()
        };
        return say(out, &line);
    }
    say(
        out,
        &format!(
            "Resources (FERB's own use over {period}; sampled every 5 minutes; CPU as a share of one core):"
        ),
    )?;
    let mut days_seen: Vec<(String, Vec<&Sample>)> = Vec::new();
    for s in &samples {
        match days_seen.iter_mut().find(|(d, _)| *d == s.day) {
            Some((_, list)) => list.push(s),
            None => days_seen.push((s.day.clone(), vec![s])),
        }
    }
    for (day, list) in &days_seen {
        let seconds: f64 = list.iter().fold(0.0, |a, r| a + r.window_s);
        // its busiest window: a short one never wins by rate
        let peak = list
            .iter()
            .fold(None::<&&Sample>, |best, r| match best {
                Some(b) if r.cpu_s <= b.cpu_s => Some(b),
                _ => Some(r),
            })
            .expect("a day has samples");
        let memory: Vec<&Sql> = list.iter().filter_map(|r| r.footprint.as_ref()).collect();
        let peaks: Vec<&Sql> = list
            .iter()
            .filter_map(|r| r.peak_footprint.as_ref())
            .collect();
        let app: Vec<&&Sample> = list.iter().filter(|r| r.app_cpu_s.is_some()).collect();
        let mut line = format!(
            "  {day}: {} h; daemon CPU avg {}, peak {} at {}; ",
            f1(seconds / 3600.0),
            share(list.iter().fold(0.0, |a, r| a + r.cpu_s), seconds),
            share(peak.cpu_s, peak.window_s),
            chars(&peak.at, 11, 16)
        );
        if !memory.is_empty() && !peaks.is_empty() {
            line.push_str(&format!(
                "memory avg {}, peak {}; ",
                shown_bytes(mean(&memory)?),
                shown_bytes(maximum(&peaks)?)
            ));
        } else {
            line.push_str("memory not read; ");
        }
        line.push_str(&format!(
            "other processes CPU avg {}; ",
            share(list.iter().fold(0.0, |a, r| a + r.child_cpu_s), seconds)
        ));
        if app.is_empty() {
            line.push_str("FERB.app not running; ");
        } else {
            line.push_str(&format!(
                "FERB.app CPU avg {}; ",
                share(
                    app.iter().fold(0.0, |a, r| a + r.app_cpu_s.unwrap_or(0.0)),
                    app.iter().fold(0.0, |a, r| a + r.window_s)
                )
            ));
        }
        let db_bytes = list
            .last()
            .and_then(|r| r.db_bytes)
            .filter(|b| *b != 0.0)
            .unwrap_or(0.0);
        line.push_str(&format!("database {}", shown_bytes(db_bytes)));
        say(out, &line)?;
    }
    say(
        out,
        "  Hottest windows (CPU of the daemon, its descendants and FERB.app):",
    )?;
    let mut hottest: Vec<&Sample> = samples.iter().collect();
    let key = |r: &Sample| -(r.cpu_s + r.child_cpu_s + r.app_cpu_s.unwrap_or(0.0));
    hottest.sort_by(|a, b| {
        key(a)
            .partial_cmp(&key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for r in hottest.iter().take(5) {
        say(out, &format!("    {}", window(r)?))?;
    }
    Ok(())
}

/// `_window`: one resource sample: its time, CPU and what FERB was doing in it (counts only).
fn window(r: &Sample) -> Res<String> {
    let work = if sql_truthy(&r.work_json) {
        loads(&r.work_json)?
    } else {
        Value::Object(Map::new())
    };
    let work = object(&work)?;
    let empty = Value::Object(Map::new());
    let events = object(work.get("events").unwrap_or(&empty))?;
    let models = object(work.get("models").unwrap_or(&empty))?;
    let perception = object(work.get("perception").unwrap_or(&empty))?;
    let batches = object(perception.get("batches").unwrap_or(&empty))?;
    let jev = object(work.get("jev").unwrap_or(&empty))?;
    let total = r.cpu_s + r.child_cpu_s + r.app_cpu_s.unwrap_or(0.0);
    let mut split = format!(
        "daemon {}, other processes {}",
        f1(r.cpu_s),
        f1(r.child_cpu_s)
    );
    if let Some(app) = r.app_cpu_s {
        split.push_str(&format!(", FERB.app {}", f1(app)));
    }
    let mut top = Vec::new();
    for pair in iterate(work.get("top"))? {
        let items = listed(&pair)?;
        let [name, cpu] = items.as_slice() else {
            return Err(format!(
                "ValueError: expected 2 values to unpack, got {}",
                items.len()
            ));
        };
        top.push(format!("{} {} s", pystr(name), f1(json_number(cpu)?)));
    }
    let top = top.join(", ");
    let zero = Value::from(0);
    let mut line = format!(
        "{} {}, {} min: {} s CPU ({split}); events {}{}; perception wakes {}, batches {}{}; Jev asked {}, \
         not sampled {}; model runs {}{}; tool calls {}; tasks running {}",
        r.day,
        chars(&r.at, 11, 16),
        f1(r.window_s / 60.0),
        f1(total),
        sum_values(events)?,
        parts(events, Some(3)),
        pystr(perception.get("wakes").unwrap_or(&zero)),
        sum_values(batches)?,
        parts(batches, None),
        pystr(jev.get("asked").unwrap_or(&zero)),
        pystr(jev.get("not_sampled").unwrap_or(&zero)),
        sum_values(models)?,
        parts(models, None),
        pystr(work.get("tool_calls").unwrap_or(&zero)),
        pystr(work.get("tasks_running").unwrap_or(&zero)),
    );
    if !top.is_empty() {
        line.push_str(&format!("; top processes {top}"));
    }
    Ok(line)
}

/// `parts(counts, top)`: the largest counts first (ties as stored), " (name n, …)", or "" when there are none.
fn parts(counts: &Map<String, Value>, top: Option<usize>) -> String {
    let mut shown: Vec<(&String, &Value)> = counts.iter().collect();
    shown.sort_by(|a, b| {
        let (x, y) = (sort_number(a.1), sort_number(b.1));
        y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(n) = top {
        shown.truncate(n);
    }
    if shown.is_empty() {
        return String::new();
    }
    let text: Vec<String> = shown
        .iter()
        .map(|(name, n)| format!("{name} {}", pystr(n)))
        .collect();
    format!(" ({})", text.join(", "))
}

fn sort_number(v: &Value) -> f64 {
    match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn sum_values(counts: &Map<String, Value>) -> Res<String> {
    let mut total = Num::I(0);
    for v in counts.values() {
        total.add(v)?;
    }
    Ok(total.shown())
}

// ---- Python's values, as `ferb stats` met them ----

/// A Python number being summed: an int until a float joins it.
enum Num {
    I(i128),
    F(f64),
}

impl Num {
    fn add(&mut self, v: &Value) -> Res<()> {
        let next = match v {
            Value::Bool(b) => Num::I(i128::from(*b)),
            Value::Number(n) if n.is_i64() || n.is_u64() => Num::I(
                n.as_i64()
                    .map_or_else(|| n.as_u64().unwrap_or(0) as i128, i128::from),
            ),
            Value::Number(n) => Num::F(n.as_f64().unwrap_or(0.0)),
            _ => return Err("TypeError: unsupported operand type(s) for +".into()),
        };
        *self = match (&*self, next) {
            (Num::I(a), Num::I(b)) => Num::I(a + b),
            (Num::I(a), Num::F(b)) => Num::F(*a as f64 + b),
            (Num::F(a), Num::I(b)) => Num::F(a + b as f64),
            (Num::F(a), Num::F(b)) => Num::F(a + b),
        };
        Ok(())
    }

    fn shown(&self) -> String {
        match self {
            Num::I(i) => i.to_string(),
            Num::F(f) => ferb_py::json::python_float(*f),
        }
    }
}

/// Keys added in order, a key's later value replacing its earlier one in place (a Python dict).
#[derive(Default)]
struct Ordered {
    at: HashMap<String, usize>,
    items: Vec<Value>,
}

impl Ordered {
    fn set(&mut self, key: String, value: Value) {
        match self.at.get(&key) {
            Some(&i) => self.items[i] = value,
            None => {
                self.at.insert(key, self.items.len());
                self.items.push(value);
            }
        }
    }

    fn values(&self) -> impl Iterator<Item = &Value> {
        self.items.iter()
    }
}

/// A decoded JSON value as Python hashes it for a dict key or set member: `1`, `1.0` and `True` are one key; a
/// list or an object can't be one.
fn hash_key(v: &Value) -> Res<String> {
    Ok(match v {
        Value::Null => "N".to_owned(),
        Value::Bool(b) => format!("n{}", u8::from(*b)),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => format!("n{i}"),
            (_, Some(u), _) => format!("n{u}"),
            (_, _, Some(f)) if f.fract() == 0.0 && f.abs() < 9.2e18 => format!("n{}", f as i64),
            (_, _, Some(f)) => format!("f{}", f.to_bits()),
            _ => "f".to_owned(),
        },
        Value::String(s) => format!("s{s}"),
        Value::Array(_) => return Err("TypeError: unhashable type: 'list'".into()),
        Value::Object(_) => return Err("TypeError: unhashable type: 'dict'".into()),
    })
}

/// `str(value)` of a decoded JSON value: text as it is, a list or an object as Python's `repr`.
fn pystr(v: &Value) -> String {
    match v {
        Value::Array(_) | Value::Object(_) => ferb_daemon::shared::py_repr(v),
        other => ferb_py::value::py_str(other),
    }
}

/// `str(d.get(key))`.
fn get_str(d: &Map<String, Value>, key: &str) -> String {
    d.get(key).map_or_else(|| "None".to_owned(), pystr)
}

/// `list(value)` of a truthy value: a list's items, a text's characters, an object's keys.
fn listed(v: &Value) -> Res<Vec<Value>> {
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(map) => Ok(map.keys().map(|k| Value::String(k.clone())).collect()),
        _ => Err("TypeError: object is not iterable".into()),
    }
}

/// `for x in d.get(key, [])`: nothing when the key is missing.
fn iterate(v: Option<&Value>) -> Res<Vec<Value>> {
    match v {
        None => Ok(Vec::new()),
        Some(v @ (Value::Array(_) | Value::String(_) | Value::Object(_))) => listed(v),
        Some(_) => Err("TypeError: object is not iterable".into()),
    }
}

/// `int(value)` of a truthy decoded value (`int(2.9)` is 2, `int("3")` is 3).
fn py_int(v: &Value) -> Res<i128> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i128::from(i))
            } else if let Some(u) = n.as_u64() {
                Ok(i128::from(u))
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if f.is_finite() {
                    Ok(f.trunc() as i128)
                } else {
                    Err("OverflowError: cannot convert float infinity to integer".into())
                }
            }
        }
        Value::String(s) => {
            let t = ferb_py::value::strip(s);
            let (sign, digits) = match t.strip_prefix('-') {
                Some(rest) => (-1, rest),
                None => (1, t.strip_prefix('+').unwrap_or(t)),
            };
            let ok = !digits.is_empty()
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__")
                && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
            let n: i128 = if ok {
                digits.replace('_', "").parse().ok()
            } else {
                None
            }
            .ok_or_else(|| {
                format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    ferb_py::text::str_repr(s)
                )
            })?;
            Ok(sign * n)
        }
        _ => Err("TypeError: int() argument must be a string or a real number".into()),
    }
}

fn json_number(v: &Value) -> Res<f64> {
    match v {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        _ => Err("ValueError: Unknown format code 'f'".into()),
    }
}

fn loads(v: &Sql) -> Res<Value> {
    match v {
        Sql::Text(t) => ferb_py::json::loads(t).map_err(|e| format!("json.JSONDecodeError: {e}")),
        _ => Err("TypeError: the JSON object must be str".into()),
    }
}

/// `json.loads(raw or '{}')`.
fn loads_or_empty(v: &Sql) -> Res<Value> {
    if sql_truthy(v) {
        loads(v)
    } else {
        Ok(Value::Object(Map::new()))
    }
}

fn object(v: &Value) -> Res<&Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| "AttributeError: object has no attribute 'get'".into())
}

/// `str()` of a value sqlite3 returned.
fn sql_str(v: &Sql) -> String {
    match v {
        Sql::Null => "None".to_owned(),
        Sql::Integer(i) => i.to_string(),
        Sql::Real(f) => ferb_py::json::python_float(*f),
        Sql::Text(t) => t.clone(),
        Sql::Blob(b) => ferb_py::text::bytes_repr(b),
    }
}

fn sql_truthy(v: &Sql) -> bool {
    match v {
        Sql::Null => false,
        Sql::Integer(i) => *i != 0,
        Sql::Real(f) => *f != 0.0,
        Sql::Text(t) => !t.is_empty(),
        Sql::Blob(b) => !b.is_empty(),
    }
}

/// `value or 0`, as printed.
fn or0(v: &Sql) -> String {
    if sql_truthy(v) {
        sql_str(v)
    } else {
        "0".to_owned()
    }
}

fn int(v: &Sql) -> i64 {
    match v {
        Sql::Integer(i) => *i,
        _ => 0,
    }
}

fn number(v: &Sql) -> Option<f64> {
    match v {
        Sql::Integer(i) => Some(*i as f64),
        Sql::Real(f) => Some(*f),
        _ => None,
    }
}

fn num_req(v: &Sql) -> Res<f64> {
    number(v).ok_or_else(|| "TypeError: unsupported operand type(s)".into())
}

/// `float(value)` of what sqlite3 returned: a number, or text Python's `float` reads.
fn py_float(v: &Sql) -> Res<f64> {
    match v {
        Sql::Integer(i) => Ok(*i as f64),
        Sql::Real(f) => Ok(*f),
        Sql::Text(t) => {
            let s = ferb_py::value::strip(t);
            let digits_around = |i: usize| {
                let b = s.as_bytes();
                i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit()
            };
            let underscores_ok = s
                .char_indices()
                .filter(|(_, c)| *c == '_')
                .all(|(i, _)| digits_around(i));
            let parsed = if underscores_ok {
                s.replace('_', "").parse::<f64>().ok()
            } else {
                None
            };
            parsed.ok_or_else(|| {
                format!(
                    "ValueError: could not convert string to float: {}",
                    ferb_py::text::str_repr(t)
                )
            })
        }
        _ => Err("TypeError: float() argument must be a string or a real number".into()),
    }
}

/// `sum(xs) / len(xs)` of stored sizes: whole bytes summed exactly, then divided.
fn mean(xs: &[&Sql]) -> Res<f64> {
    if xs.iter().all(|x| matches!(x, Sql::Integer(_))) {
        let total: i128 = xs.iter().map(|x| i128::from(int(x))).sum();
        return Ok(total as f64 / xs.len() as f64);
    }
    let mut total = 0.0;
    for x in xs {
        total += num_req(x)?;
    }
    Ok(total / xs.len() as f64)
}

/// `max(xs)`: the first largest.
fn maximum(xs: &[&Sql]) -> Res<f64> {
    let mut best: Option<f64> = None;
    for x in xs {
        let v = num_req(x)?;
        if best.is_none_or(|b| v > b) {
            best = Some(v);
        }
    }
    Ok(best.unwrap_or(0.0))
}

/// `f"{x:.1f}"`.
fn f1(x: f64) -> String {
    fixed(x, 1)
}

/// `f"{x:.Nf}"`: Rust's fixed notation rounds the exact binary value half to even, as Python does.
fn fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        "nan".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 { "inf" } else { "-inf" }.to_owned()
    } else {
        format!("{x:.digits$}")
    }
}

/// `f"{text:<width}"`, by characters.
fn ljust(text: &str, width: usize) -> String {
    let n = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(n)))
}

/// `f"{text:>width}"`, by characters.
fn rjust(text: &str, width: usize) -> String {
    let n = text.chars().count();
    format!("{}{text}", " ".repeat(width.saturating_sub(n)))
}

/// `text[a:b]` by characters.
fn chars(text: &str, a: usize, b: usize) -> String {
    text.chars().skip(a).take(b - a).collect()
}

/// `re.sub(r"\d+", "#", text)`: every run of Unicode decimal digits as one "#".
fn numbers_set_aside(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for ch in text.chars() {
        if ferb_py::unicode::is_decimal(ch) {
            if !in_run {
                out.push('#');
            }
            in_run = true;
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out
}

fn rows(c: &Connection, sql: &str, params: &[Sql]) -> Res<Vec<Vec<Sql>>> {
    let mut st = c.prepare(sql).map_err(sq)?;
    let n = st.column_count();
    let mut found = Vec::new();
    let mut it = st
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(sq)?;
    while let Some(r) = it.next().map_err(sq)? {
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            row.push(r.get::<_, Sql>(i).map_err(sq)?);
        }
        found.push(row);
    }
    Ok(found)
}

fn one(c: &Connection, sql: &str, params: &[Sql]) -> Res<Vec<Sql>> {
    rows(c, sql, params)?
        .into_iter()
        .next()
        .ok_or_else(|| "TypeError: cannot unpack non-iterable NoneType object".into())
}
