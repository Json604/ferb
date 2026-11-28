//! ORCH-719 item 3's weekly guard (ORCH-756), a section of `ferb stats`: code only, no model call. Over the last 7
//! days' worker runs that recorded their tools (`tool_core` on the run's span: the tools listed in full and those
//! listed by line, `Gateway::run_listing`), it flags a run whose answer said not done (its span's `status`
//! `not_done`) while a tool it had listed by line only, and never called, fits the task's targets structurally:
//! a site target fits the browser tools (`browser.*`, `ferb_browser.*`), an app target the Mac tools (`mac.*`), a
//! path target the file tools (`fs.*`). The targets are the task tree's (`task_context.spec_json`). Nothing else is
//! judged: not why the run stopped, not whether the tool would have helped. Printed only when such runs were
//! recorded in the week.

use std::collections::BTreeSet;
use std::io::Write;

use rusqlite::types::Value as Sql;
use rusqlite::Connection;
use serde_json::Value;

type Res<T> = Result<T, String>;

/// The days the guard looks back.
pub const DAYS: i64 = 7;

fn sq(e: rusqlite::Error) -> String {
    format!("sqlite3.OperationalError: {e}")
}

fn text(v: &Sql) -> String {
    match v {
        Sql::Text(t) => t.clone(),
        _ => String::new(),
    }
}

/// The tool families a task's targets fit (`targets` as the spec stores it).
pub fn fitting(targets: &Value) -> Vec<(&'static str, &'static [&'static str])> {
    let has = |k: &str| {
        targets[k].as_array().is_some_and(|a| {
            a.iter()
                .any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
        })
    };
    let mut out: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if has("sites") {
        out.push(("site", &["browser.", "ferb_browser."]));
    }
    if has("apps") {
        out.push(("app", &["mac."]));
    }
    if has("paths") {
        out.push(("path", &["fs."]));
    }
    out
}

/// The section (`since`: an ISO instant DAYS back, as the other sections count back).
pub fn report(c: &Connection, since: &str, out: &mut dyn Write) -> Res<()> {
    let tables: BTreeSet<String> = {
        let mut s = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .map_err(sq)?;
        let names = s
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(sq)?
            .collect::<Result<_, _>>()
            .map_err(sq)?;
        names
    };
    if !["spans", "tasks", "task_context"]
        .iter()
        .all(|t| tables.contains(*t))
    {
        return Ok(());
    }
    let mut runs = c
        .prepare(
            "SELECT s.id, s.task_id, json_extract(s.input_json, '$.tool_core.listed'), \
             json_extract(s.output_json, '$.status'), \
             (SELECT json_extract(x.spec_json, '$.targets') FROM tasks t JOIN task_context x ON x.task_id = t.tree_id \
              WHERE t.id = s.task_id) \
             FROM spans s WHERE s.started_at >= ? AND json_extract(s.input_json, '$.tool_core.kind') = 'worker' \
             ORDER BY s.started_at, s.rowid",
        )
        .map_err(sq)?;
    let rows: Vec<Vec<Sql>> = runs
        .query_map([since], |r| (0..5).map(|i| r.get::<_, Sql>(i)).collect())
        .map_err(sq)?
        .collect::<Result<_, _>>()
        .map_err(sq)?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut called = c
        .prepare(
            "SELECT DISTINCT json_extract(input_json, '$.tool') FROM spans WHERE parent_id = ? AND kind = 'tool.call'",
        )
        .map_err(sq)?;
    let mut not_done = 0;
    let mut flagged: Vec<String> = Vec::new();
    for r in &rows {
        if text(&r[3]) != "not_done" {
            continue;
        }
        not_done += 1;
        let listed: Vec<String> = serde_json::from_str::<Value>(&text(&r[2]))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let targets: Value = serde_json::from_str(&text(&r[4])).unwrap_or(Value::Null);
        let used: BTreeSet<String> = called
            .query_map([text(&r[0])], |x| x.get::<_, Option<String>>(0))
            .map_err(sq)?
            .filter_map(|x| x.ok().flatten())
            .collect();
        let mut fits: Vec<String> = Vec::new();
        for (kind, prefixes) in fitting(&targets) {
            let tools: Vec<&String> = listed
                .iter()
                .filter(|t| prefixes.iter().any(|p| t.starts_with(p)) && !used.contains(*t))
                .collect();
            if !tools.is_empty() {
                fits.push(format!(
                    "{kind}: {}",
                    tools
                        .iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        if !fits.is_empty() {
            flagged.push(format!(
                "  ferb trace {} — {}",
                text(&r[1]),
                fits.join("; ")
            ));
        }
    }
    let say = |out: &mut dyn Write, line: &str| -> Res<()> {
        writeln!(out, "{line}").map_err(|e| format!("BrokenPipeError: {e}"))
    };
    say(
        out,
        &format!(
            "Tools listed by line (ORCH-719 weekly guard, last {DAYS} days): {} worker run(s), {not_done} said not \
             done, {} of them with a fitting listed tool never called",
            rows.len(),
            flagged.len()
        ),
    )?;
    for f in flagged {
        say(out, &f)?;
    }
    Ok(())
}
