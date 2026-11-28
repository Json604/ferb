//! `ferb stats`' model-use section (ORCH-720, for ORCH-719 step 1): per role and runtime, and per day, what FERB's
//! model calls cost in the provider's own token counts, read only from the `model.usage` spans and the spans they
//! hang under (the run's `worker` or `model.turn` span, its tool calls and its `runtime.tools` record). The task's
//! request id (from `tasks`) and the scheduled runs' request ids (from the journal's `schedule.run` rows) only sort a
//! record into a role.
//!
//! - **Roles:** planner (a chat turn), worker and checker (a worker run, by its role), scheduled and temporary (any
//!   run of a scheduled run's or a temporary chat's request), memory/learning (a call with no parent run), other.
//! - **Calls:** model requests. Codex reports each call (one record each). Claude Code reports one total per turn;
//!   FERB's relay names the turn's requests (RUN-477, `requests`); an older record without it is estimated as its
//!   run's tool calls plus one per turn, and the row says so with `~` (RUN-421's normalizing).
//! - **Steps:** the tool calls made in those runs; per step is input over steps.
//! - **Fixed part:** the run's instructions (`system_prompt`, the median over its runs, in characters) and the tools
//!   the provider saw offered (`runtime.tools`, the median count). The tool descriptions' size is not in the spans.
//!
//! Then, each only when its records exist in the range (ORCH-726 step 1, T-1338):
//!
//! - **Model call time** per role and runtime: each record's own `timing` (RUN-419, RUN-420), the median to the first
//!   output and of the whole call; records whose clock stood still (every figure 0, before journal 254) are left out.
//! - **Claude relay requests** per role and runtime (RUN-479): requests recorded and let go, tools called, results and
//!   how many were errors, the tools called again in one run (and how many of those after an error result), and the
//!   requests' median time to the first byte.
//! - **Time to the first word** ([`first_words`], ORCH-732): per door (the front door or the planner) and runtime, each
//!   chat turn's `first_text_ms`, the median, the 90th percentile and the slowest.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;

use rusqlite::types::Value as Sql;
use rusqlite::Connection;
use serde_json::Value;

type Res<T> = Result<T, String>;

/// Which records the section reads.
pub enum Range {
    /// `started_at >= since` (an ISO instant, as the other sections count back `--days`).
    Since { since: String, days: i64 },
    /// Local days, both included (`--from`/`--to`, YYYY-MM-DD), as recorded in each span's own time.
    Days { from: String, to: String },
}

impl Range {
    fn clause(&self) -> (&'static str, Vec<Sql>) {
        match self {
            Range::Since { since, .. } => ("u.started_at >= ?", vec![Sql::Text(since.clone())]),
            Range::Days { from, to } => (
                "substr(u.started_at, 1, 10) BETWEEN ? AND ?",
                vec![Sql::Text(from.clone()), Sql::Text(to.clone())],
            ),
        }
    }

    fn label(&self) -> String {
        match self {
            Range::Since { days, .. } => format!("in the last {days} day(s)"),
            Range::Days { from, to } if from == to => format!("on {from}"),
            Range::Days { from, to } => format!("from {from} to {to}"),
        }
    }
}

fn sq(e: rusqlite::Error) -> String {
    format!("sqlite3.OperationalError: {e}")
}

fn text(v: &Sql) -> String {
    match v {
        Sql::Text(t) => t.clone(),
        Sql::Integer(i) => i.to_string(),
        Sql::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

fn int(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        _ => 0,
    }
}

/// `1234567` as "1,234,567".
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

fn share(part: i64, whole: i64) -> String {
    if whole <= 0 {
        "0%".into()
    } else {
        format!("{}%", (100 * part) / whole)
    }
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// One usage record with what it hangs under.
struct Record {
    parent: Option<String>,
    own: String,
    day: String,
    runtime: String,
    role: String,
    usage: serde_json::Map<String, Value>,
}

#[derive(Default)]
struct Sum {
    records: i64,
    runs: HashSet<String>,
    calls: i64,
    estimated: bool,
    steps: i64,
    input: i64,
    cached: i64,
    write_5m: i64,
    write_1h: i64,
    output: i64,
    at_end: i64,
    instructions: Vec<i64>,
    tools: Vec<i64>,
}

impl Sum {
    fn per(&self, what: i64) -> String {
        if what <= 0 {
            "-".into()
        } else {
            thousands(self.input / what)
        }
    }
}

/// The section; nothing is printed when no usage was recorded in the range.
pub fn report(c: &Connection, range: &Range, out: &mut dyn Write) -> Res<()> {
    let tables: HashSet<String> =
        query(c, "SELECT name FROM sqlite_master WHERE type='table'", &[])?
            .into_iter()
            .map(|r| text(&r[0]))
            .collect();
    if !tables.contains("spans") {
        return Ok(());
    }
    let (clause, params) = range.clause();
    let rows = query(
        c,
        &format!(
            "SELECT u.id, u.parent_id, substr(u.started_at, 1, 10), json_extract(u.input_json, '$.runtime'), \
             json_extract(u.input_json, '$.role'), u.output_json, p.kind, json_extract(p.input_json, '$.role'), \
             json_extract(p.input_json, '$.request_id'), p.task_id, \
             length(json_extract(p.input_json, '$.system_prompt')) \
             FROM spans u LEFT JOIN spans p ON p.id = u.parent_id \
             WHERE u.kind = 'model.usage' AND {clause} ORDER BY u.started_at, u.rowid"
        ),
        &params,
    )?;
    if rows.is_empty() {
        return Ok(());
    }
    // which request each task and scheduled run belongs to
    let mut request_of: HashMap<String, String> = HashMap::new();
    if tables.contains("tasks") {
        for r in query(c, "SELECT id, request_id FROM tasks", &[])? {
            request_of.insert(text(&r[0]), text(&r[1]));
        }
    }
    let mut scheduled: HashSet<String> = HashSet::new();
    if tables.contains("journal") {
        for r in query(
            c,
            "SELECT json_extract(data_json, '$.request_id') FROM journal WHERE kind = 'schedule.run'",
            &[],
        )? {
            scheduled.insert(text(&r[0]));
        }
    }
    // per parent run: its tool calls, the tools the provider saw, its instructions' size
    let parents: HashSet<String> = rows
        .iter()
        .map(|r| text(&r[1]))
        .filter(|p| !p.is_empty())
        .collect();
    let mut steps: HashMap<String, i64> = HashMap::new();
    let mut tools: HashMap<String, i64> = HashMap::new();
    for r in query(
        c,
        &format!(
            "SELECT parent_id, kind, COUNT(*), MAX(json_array_length(output_json, '$.tools_seen')) FROM spans \
             WHERE kind IN ('tool.call', 'runtime.tools') AND parent_id IN (SELECT u.parent_id FROM spans u \
             WHERE u.kind = 'model.usage' AND {clause}) GROUP BY parent_id, kind"
        ),
        &params,
    )? {
        let p = text(&r[0]);
        if !parents.contains(&p) {
            continue;
        }
        match text(&r[1]).as_str() {
            "tool.call" => {
                steps.insert(p, match r[2] {
                    Sql::Integer(n) => n,
                    _ => 0,
                });
            }
            _ => {
                if let Sql::Integer(n) = r[3] {
                    tools.insert(p, n);
                }
            }
        }
    }
    let mut instructions: HashMap<String, i64> = HashMap::new();
    let mut records = Vec::new();
    for r in &rows {
        let parent = Some(text(&r[1])).filter(|p| !p.is_empty());
        let kind = text(&r[6]);
        let request = match kind.as_str() {
            "model.turn" => text(&r[8]),
            "worker" => request_of.get(&text(&r[9])).cloned().unwrap_or_default(),
            _ => String::new(),
        };
        let role = if request.starts_with("temporary-") {
            "temporary"
        } else if !request.is_empty() && scheduled.contains(&request) {
            "scheduled"
        } else {
            match (kind.as_str(), text(&r[7]).as_str()) {
                ("model.turn", _) => "planner",
                ("worker", "verifier") => "checker",
                ("worker", _) => "worker",
                ("", _) if parent.is_none() => "memory/learning",
                _ => "other",
            }
        };
        if let (Some(p), Sql::Integer(n)) = (&parent, &r[10]) {
            instructions.insert(p.clone(), *n);
        }
        let usage = match serde_json::from_str::<Value>(&text(&r[5])) {
            Ok(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        let runtime = Some(text(&r[3]))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "?".into());
        records.push(Record {
            parent,
            own: text(&r[0]),
            day: text(&r[2]),
            runtime,
            role: role.to_string(),
            usage,
        });
    }
    // Claude records without the relay's request count: their run's tool calls plus one per turn, shared out once
    let mut unnamed: HashMap<String, i64> = HashMap::new();
    for rec in &records {
        if rec.runtime == "claude" && !rec.usage.contains_key("requests") {
            if let Some(p) = &rec.parent {
                *unnamed.entry(p.clone()).or_default() += 1;
            }
        }
    }
    let mut first_unnamed: HashSet<String> = HashSet::new();
    let calls_of = |rec: &Record, first: &mut HashSet<String>| -> (i64, bool) {
        if let Some(n) = rec.usage.get("requests").and_then(Value::as_i64) {
            return (n, false);
        }
        if rec.runtime != "claude" {
            return (1, false);
        }
        match &rec.parent {
            Some(p) if first.insert(p.clone()) => {
                (steps.get(p).copied().unwrap_or(0) + unnamed[p], true)
            }
            Some(_) => (0, true),
            // a call with no run of its own (a learning pass) is one request with no tools
            None => (1, false),
        }
    };
    let roles = [
        "planner",
        "worker",
        "checker",
        "scheduled",
        "temporary",
        "memory/learning",
        "other",
    ];
    let mut by_role: BTreeMap<(usize, String), Sum> = BTreeMap::new();
    let mut by_day: BTreeMap<(String, usize, String), Sum> = BTreeMap::new();
    let mut seen_steps: HashSet<String> = HashSet::new();
    let mut seen_day_steps: HashSet<(String, String)> = HashSet::new();
    for rec in &records {
        let at = roles
            .iter()
            .position(|r| *r == rec.role)
            .unwrap_or(roles.len() - 1);
        let (calls, estimated) = calls_of(rec, &mut first_unnamed);
        let run = rec.parent.clone().unwrap_or_else(|| rec.own.clone());
        for (sum, first_step) in [
            (
                by_role.entry((at, rec.runtime.clone())).or_default(),
                seen_steps.insert(run.clone()),
            ),
            (
                by_day
                    .entry((rec.day.clone(), at, rec.runtime.clone()))
                    .or_default(),
                seen_day_steps.insert((rec.day.clone(), run.clone())),
            ),
        ] {
            sum.records += 1;
            sum.runs.insert(run.clone());
            sum.calls += calls;
            sum.estimated |= estimated;
            if first_step {
                sum.steps += steps.get(&run).copied().unwrap_or(0);
                if let Some(n) = instructions.get(&run) {
                    sum.instructions.push(*n);
                }
                if let Some(n) = tools.get(&run) {
                    sum.tools.push(*n);
                }
            }
            sum.input += int(rec.usage.get("input_tokens"));
            sum.cached += int(rec.usage.get("cached_input_tokens"));
            sum.write_5m += int(rec.usage.get("cache_write_5m_tokens"));
            sum.write_1h += int(rec.usage.get("cache_write_1h_tokens"));
            sum.output += int(rec.usage.get("output_tokens"));
            if rec.usage.get("recorded_at_end") == Some(&Value::Bool(true)) {
                sum.at_end += 1;
            }
        }
    }
    say(
        out,
        &format!(
            "Model use by role and runtime {} (the providers' own token counts):",
            range.label()
        ),
    )?;
    let (mut total, mut estimated, mut at_end) = (0i64, false, 0i64);
    for ((at, runtime), s) in &by_role {
        total += s.input;
        estimated |= s.estimated;
        at_end += s.at_end;
        let mut fixed = Vec::new();
        if let Some(m) = median(s.instructions.clone()) {
            fixed.push(format!("instructions {} chars", thousands(m)));
        }
        if let Some(m) = median(s.tools.clone()) {
            fixed.push(format!("{m} tools seen"));
        }
        say(
            out,
            &format!(
                "  {}, {runtime}: {} runs, {}{} calls, {} steps; input {} ({} per call, {} per step); cached {}; \
                 cache written 5m {}, 1h {}; output {}{}",
                roles[*at],
                thousands(s.runs.len() as i64),
                if s.estimated { "~" } else { "" },
                thousands(s.calls),
                thousands(s.steps),
                thousands(s.input),
                s.per(s.calls),
                s.per(s.steps),
                share(s.cached, s.input),
                thousands(s.write_5m),
                thousands(s.write_1h),
                thousands(s.output),
                if fixed.is_empty() {
                    String::new()
                } else {
                    format!("; fixed part: {}", fixed.join(", "))
                }
            ),
        )?;
    }
    say(out, &format!("  all: input {}", thousands(total)))?;
    if estimated {
        say(
            out,
            "  ~ estimated: an older Claude record holds one total per turn, counted as its run's tool calls plus one",
        )?;
    }
    if at_end > 0 {
        say(
            out,
            &format!("  {at_end} records counted at a run's end (it stopped before its runtime reported them)"),
        )?;
    }
    say(out, "Model use by day:")?;
    for ((day, at, runtime), s) in &by_day {
        say(
            out,
            &format!(
                "  {day} {}, {runtime}: {}{} calls, input {} ({} per call), cached {}, output {}",
                roles[*at],
                if s.estimated { "~" } else { "" },
                thousands(s.calls),
                thousands(s.input),
                s.per(s.calls),
                share(s.cached, s.input),
                thousands(s.output),
            ),
        )?;
    }
    call_times(&records, &roles, out)?;
    relay_requests(&records, &roles, out)
}

/// Milliseconds as seconds with two decimals ("1.68 s").
fn secs(ms: i64) -> String {
    format!("{:.2} s", ms as f64 / 1000.0)
}

/// The `p`th percentile (nearest rank) of a sorted list.
fn rank(sorted: &[i64], p: f64) -> i64 {
    let at = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[at]
}

fn role_at(roles: &[&str], role: &str) -> usize {
    roles
        .iter()
        .position(|r| *r == role)
        .unwrap_or(roles.len() - 1)
}

/// Model call time per role and runtime: each record's own timing; a record whose clock stood still is left out.
fn call_times(records: &[Record], roles: &[&str], out: &mut dyn Write) -> Res<()> {
    let mut by: BTreeMap<(usize, String), (Vec<i64>, Vec<i64>)> = BTreeMap::new();
    for rec in records {
        let Some(Value::Object(t)) = rec.usage.get("timing") else {
            continue;
        };
        let total = int(t.get("total_ms"));
        if total <= 0 {
            continue;
        }
        let e = by
            .entry((role_at(roles, &rec.role), rec.runtime.clone()))
            .or_default();
        e.1.push(total);
        if t.contains_key("ttft_ms") {
            e.0.push(int(t.get("ttft_ms")));
        }
    }
    if by.is_empty() {
        return Ok(());
    }
    say(
        out,
        "Model call time by role and runtime (each record's own clock; a record whose clock stood still is left out):",
    )?;
    for ((at, runtime), (ttft, total)) in by {
        let first = median(ttft).map_or_else(String::new, |m| {
            format!(", to the first output median {}", secs(m))
        });
        say(
            out,
            &format!(
                "  {}, {runtime}: {} records{first}, whole call median {}",
                roles[at],
                thousands(total.len() as i64),
                secs(median(total).unwrap_or(0))
            ),
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct Relay {
    records: i64,
    requests: i64,
    dropped: i64,
    called: i64,
    results: i64,
    errors: i64,
    /// tool name → (called again in one run, of those after an error result for it)
    again: BTreeMap<String, (i64, i64)>,
    first_byte: Vec<i64>,
}

/// FERB's Claude relay's record of each request (RUN-479), per role and runtime: so a doubled answer names its cause
/// (a tool called again after an error result is a refusal answered; again after no error, the runtime's own retry).
fn relay_requests(records: &[Record], roles: &[&str], out: &mut dyn Write) -> Res<()> {
    let mut by: BTreeMap<(usize, String), Relay> = BTreeMap::new();
    for rec in records {
        let Some(Value::Array(steps)) = rec.usage.get("steps") else {
            continue;
        };
        let r = by
            .entry((role_at(roles, &rec.role), rec.runtime.clone()))
            .or_default();
        r.records += 1;
        r.requests += steps.len() as i64;
        r.dropped += int(rec.usage.get("steps_dropped"));
        let mut seen: HashSet<String> = HashSet::new();
        for step in steps {
            let results = step
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            r.results += results.len() as i64;
            r.errors += results
                .iter()
                .filter(|x| x.get("error") == Some(&Value::Bool(true)))
                .count() as i64;
            if let Some(ms) = step.get("first_byte_ms").and_then(Value::as_i64) {
                r.first_byte.push(ms);
            }
            for name in step
                .get("called")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                r.called += 1;
                if !seen.insert(name.to_string()) {
                    let after_error = results.iter().any(|x| {
                        x.get("tool").and_then(Value::as_str) == Some(name)
                            && x.get("error") == Some(&Value::Bool(true))
                    });
                    let e = r.again.entry(name.to_string()).or_default();
                    e.0 += 1;
                    e.1 += i64::from(after_error);
                }
            }
        }
    }
    if by.is_empty() {
        return Ok(());
    }
    say(
        out,
        "Claude relay requests by role and runtime (RUN-479: per request the tools called and each result's error flag):",
    )?;
    for ((at, runtime), r) in by {
        let let_go = if r.dropped > 0 {
            format!(" (and {} not kept)", thousands(r.dropped))
        } else {
            String::new()
        };
        let again = if r.again.is_empty() {
            "none".to_string()
        } else {
            r.again
                .iter()
                .map(|(name, (n, after))| format!("{name} {n} ({after} after an error)"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let first = median(r.first_byte)
            .map_or_else(String::new, |m| format!("; first byte median {}", secs(m)));
        say(
            out,
            &format!(
                "  {}, {runtime}: {} records, {} requests{let_go}; tools called {}, results {} ({} errors); \
                 called again in one run: {again}{first}",
                roles[at],
                thousands(r.records),
                thousands(r.requests),
                thousands(r.called),
                thousands(r.results),
                thousands(r.errors),
            ),
        )?;
    }
    Ok(())
}

/// ORCH-732: how long the owner waited for each chat turn's first word (`first_text_ms` of the `model.turn` span that
/// sent it, from its intake), per door and runtime (the runtime of the turn's own usage record, else of its
/// `model.close` record: a run ended on its accepted call may report no usage); nothing when no turn in the range
/// recorded one.
pub fn first_words(c: &Connection, range: &Range, out: &mut dyn Write) -> Res<()> {
    let tables: HashSet<String> =
        query(c, "SELECT name FROM sqlite_master WHERE type='table'", &[])?
            .into_iter()
            .map(|r| text(&r[0]))
            .collect();
    if !tables.contains("spans") {
        return Ok(());
    }
    let (clause, params) = range.clause();
    let rows = query(
        c,
        &format!(
            "SELECT json_extract(u.output_json, '$.timing.first_text_ms'), json_extract(u.input_json, '$.front_door'), \
             (SELECT json_extract(m.input_json, '$.runtime') FROM spans m WHERE m.parent_id = u.id AND \
              m.kind IN ('model.usage', 'model.close') AND json_extract(m.input_json, '$.runtime') IS NOT NULL \
              ORDER BY m.kind = 'model.close', m.started_at, m.rowid LIMIT 1) \
             FROM spans u WHERE u.kind = 'model.turn' AND {clause} \
             AND json_extract(u.output_json, '$.timing.first_text_ms') IS NOT NULL"
        ),
        &params,
    )?;
    let mut by: BTreeMap<(u8, String), Vec<i64>> = BTreeMap::new();
    for r in &rows {
        let Sql::Integer(ms) = r[0] else { continue };
        let front = matches!(r[1], Sql::Integer(1));
        let runtime = Some(text(&r[2]))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "?".into());
        by.entry((u8::from(!front), runtime)).or_default().push(ms);
    }
    if by.is_empty() {
        return Ok(());
    }
    say(
        out,
        &format!(
            "Time to the first word {} (chat turns, from FERB's intake of the message to the first word shown):",
            range.label()
        ),
    )?;
    for ((door, runtime), mut ms) in by {
        ms.sort_unstable();
        say(
            out,
            &format!(
                "  {}, {runtime}: {} turns, median {}, 90% within {}, slowest {}",
                if door == 0 { "front door" } else { "planner" },
                thousands(ms.len() as i64),
                secs(median(ms.clone()).unwrap_or(0)),
                secs(rank(&ms, 0.9)),
                secs(*ms.last().unwrap_or(&0)),
            ),
        )?;
    }
    Ok(())
}

/// ORCH-760, a section of `ferb stats`: the worker runs FERB ended for a fresh one in the range (their
/// `worker.fresh_run` records), the attempts they were in, the median size they had reached, and the attempts that
/// passed 5 (the alarm), the newest named for `ferb trace`. Printed only when such runs were recorded.
pub fn fresh_runs(c: &Connection, range: &Range, out: &mut dyn Write) -> Res<()> {
    let tables: HashSet<String> =
        query(c, "SELECT name FROM sqlite_master WHERE type='table'", &[])?
            .into_iter()
            .map(|r| text(&r[0]))
            .collect();
    if !tables.contains("spans") {
        return Ok(());
    }
    let (clause, params) = range.clause();
    let rows = query(
        c,
        &format!(
            "SELECT coalesce(u.task_id, ''), coalesce(u.parent_id, ''),              json_extract(u.input_json, '$.context_tokens'), json_extract(u.output_json, '$.alarm')              FROM spans u WHERE u.kind = 'worker.fresh_run' AND {clause} ORDER BY u.started_at, u.rowid"
        ),
        &params,
    )?;
    if rows.is_empty() {
        return Ok(());
    }
    let attempts: HashSet<(String, String)> =
        rows.iter().map(|r| (text(&r[0]), text(&r[1]))).collect();
    let sizes: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r[2] {
            Sql::Integer(n) => Some(n),
            _ => None,
        })
        .collect();
    let alarmed: Vec<&Vec<Sql>> = rows
        .iter()
        .filter(|r| matches!(r[3], Sql::Integer(1)))
        .collect();
    let alarm_attempts: HashSet<(String, String)> =
        alarmed.iter().map(|r| (text(&r[0]), text(&r[1]))).collect();
    say(
        out,
        &format!(
            "Fresh runs {} (worker runs FERB ended past the size and went on with a hand-off note):",
            range.label()
        ),
    )?;
    say(
        out,
        &format!(
            "  {} fresh runs in {} attempts, median size reached {} tokens",
            thousands(rows.len() as i64),
            thousands(attempts.len() as i64),
            thousands(median(sizes).unwrap_or(0))
        ),
    )?;
    if let Some(last) = alarmed.last() {
        say(
            out,
            &format!(
                "  past 5 in one attempt (the alarm): {} attempts; the newest: ferb trace {}",
                thousands(alarm_attempts.len() as i64),
                text(&last[0])
            ),
        )?;
    }
    Ok(())
}

fn say(out: &mut dyn Write, line: &str) -> Res<()> {
    writeln!(out, "{line}").map_err(|e| format!("BrokenPipeError: {e}"))
}

fn query(c: &Connection, sql: &str, params: &[Sql]) -> Res<Vec<Vec<Sql>>> {
    let mut st = c.prepare(sql).map_err(sq)?;
    let n = st.column_count();
    let rows = st
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            (0..n)
                .map(|i| r.get::<_, Sql>(i))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sq)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sq)
}
