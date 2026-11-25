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
    let say = |out: &mut dyn Write, line: &str| -> Res<()> {
        writeln!(out, "{line}").map_err(|e| format!("BrokenPipeError: {e}"))
    };
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
    Ok(())
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
