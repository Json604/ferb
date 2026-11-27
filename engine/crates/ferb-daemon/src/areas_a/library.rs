//! The Librarian (kb/plans/front-door.md "After the hand-off" → Librarian; the owner, 2026-11-26: "no, then add a daily
//! librarian call. one call isnt taht much anyways"): code lists what FERB recorded each day; one model call a day
//! writes a closed day's topic lines (ferb-learning `library`), one a month that month's line from its day lines, one
//! a year that year's line. Run by the hourly memory pass, never during a turn.
//!
//! The rate, for any user (agent mechanism within the owner's "one call a day"):
//! - at most one day call per day on the Mac's calendar, and only when a closed day of the last 14 had a message of
//!   the owner's and has no lines yet (newest such day first: a Mac that was off catches up one day a day while the
//!   day is still in the index); none on a silent day, none when every day is written;
//! - at most one month call per day, only for a closed month with day lines and no line of its own: about one a month;
//! - at most one year call per day, only for a closed year whose months are all written: about one a year.
//!
//!   So at most 3 calls on the busiest day (the first of January), and about 30 + 1 + 0.1 a month for a user who talks
//!   to FERB every day; a failed call counts as that day's call and is tried again the next day.
//!
//! Each call is on the smallest model of the owner's chosen provider (the runtimes area's role
//! [`LIBRARIAN_ROLE`]), its span `model.learning` and usage `model.usage` with the role "librarian". It reads only
//! his chats' entries without outside content and code's list, never his coding sessions (MEM-466) or an import's
//! lines (MEM-489).

use std::rc::Rc;

use ferb_learning::library::{self, Input, DAY_KIND, FAILED_SUFFIX, MONTH_KIND, YEAR_KIND};
use serde_json::{json, Value};

use super::memory::LearningSpans;
use super::State;

/// The runtimes area's role for the Librarian's calls: the orchestrator's runtime on its smallest model.
pub const LIBRARIAN_ROLE: &str = "librarian";

/// What one pass did: each period written (`"day 2026-11-26"`, ...) and each that failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pass {
    pub written: Vec<String>,
    pub failed: Vec<String>,
    /// Model calls made.
    pub calls: usize,
}

/// The local day ("YYYY-MM-DD") of a stored ISO time on this Mac's calendar.
pub fn day_of(st: &State) -> impl Fn(&str) -> Option<String> {
    let zone = st.clock.zone();
    move |at: &str| {
        ferb_clock::py::parse(at).map(|t| ferb_clock::py::fmt(&zone.local(&t), "%Y-%m-%d"))
    }
}

/// The local clock time ("HH:MM") of a stored ISO time.
pub fn clock_of(st: &State) -> impl Fn(&str) -> Option<String> {
    let zone = st.clock.zone();
    move |at: &str| ferb_clock::py::parse(at).map(|t| ferb_clock::py::fmt(&zone.local(&t), "%H:%M"))
}

/// Today on this Mac's calendar.
pub fn today(st: &State) -> String {
    let zone = st.clock.zone();
    ferb_clock::py::fmt(&zone.local(&st.now()), "%Y-%m-%d")
}

/// The Librarian's hourly turn: the day call, the month call and the year call when each is due (module docs).
pub async fn librarian_pass(st: &Rc<State>) -> ferb_learning::Result<Pass> {
    let mut pass = Pass::default();
    if st.make_runtime.is_none() {
        return Ok(pass);
    }
    let today = today(st);
    let day_of = day_of(st);
    let clock_of = clock_of(st);
    // the day
    let conn = st.db().conn();
    if !library::called_today(conn, DAY_KIND, &today, &day_of)? {
        let done = library::written(conn, DAY_KIND, "day", false)?;
        let due = library::days_with_messages(conn, &today, &day_of)?
            .into_iter()
            .find(|d| !done.contains(d));
        if let Some(day) = due {
            if let Some(input) = library::day_input(conn, &day, &day_of, &clock_of)? {
                pass.calls += 1;
                match call(st, "day", &input, &library::day_schema()).await {
                    Ok(answer) => {
                        let lines = library::day_lines(&answer.0, &input);
                        record(
                            st,
                            DAY_KIND,
                            json!({"day": day, "lines": lines, "made_by": answer.1}),
                            &input,
                        )?;
                        pass.written.push(format!("day {day}"));
                    }
                    Err(e) => {
                        failed(st, DAY_KIND, "day", &day, &e)?;
                        pass.failed.push(format!("day {day}"));
                    }
                }
            }
        }
    }
    let conn = st.db().conn();
    // the month: a closed month with day lines and no line of its own
    let this_month = today[..7].to_owned();
    if !library::called_today(conn, MONTH_KIND, &today, &day_of)? {
        let done = library::written(conn, MONTH_KIND, "month", false)?;
        let mut months: Vec<String> = library::written(conn, DAY_KIND, "day", false)?
            .iter()
            .map(|d| d[..7].to_owned())
            .filter(|m| m.as_str() < this_month.as_str() && !done.contains(m))
            .collect();
        months.sort();
        months.dedup();
        if let Some(month) = months.pop() {
            if let Some(input) = library::month_input(conn, &month)? {
                pass.calls += 1;
                period(st, MONTH_KIND, "month", &month, &input, &mut pass).await?;
            }
        }
    }
    let conn = st.db().conn();
    // the year: a closed year whose months with day lines are all written
    let this_year = today[..4].to_owned();
    if !library::called_today(conn, YEAR_KIND, &today, &day_of)? {
        let done = library::written(conn, YEAR_KIND, "year", false)?;
        let months = library::written(conn, MONTH_KIND, "month", false)?;
        let day_months: std::collections::BTreeSet<String> =
            library::written(conn, DAY_KIND, "day", false)?
                .iter()
                .map(|d| d[..7].to_owned())
                .collect();
        let mut years: Vec<String> = months
            .iter()
            .map(|m| m[..4].to_owned())
            .filter(|y| y.as_str() < this_year.as_str() && !done.contains(y))
            .filter(|y| {
                day_months
                    .iter()
                    .filter(|m| m.starts_with(&format!("{y}-")))
                    .all(|m| months.contains(m))
            })
            .collect();
        years.sort();
        years.dedup();
        if let Some(year) = years.pop() {
            if let Some(input) = library::year_input(conn, &year)? {
                pass.calls += 1;
                period(st, YEAR_KIND, "year", &year, &input, &mut pass).await?;
            }
        }
    }
    Ok(pass)
}

async fn period(
    st: &Rc<State>,
    kind: &str,
    key: &str,
    which: &str,
    input: &Input,
    pass: &mut Pass,
) -> ferb_learning::Result<()> {
    match call(st, key, input, &library::period_schema()).await {
        Ok(answer) => {
            let line = library::period_line(&answer.0);
            let mut data = serde_json::Map::new();
            data.insert(key.into(), json!(which));
            data.insert("text".into(), json!(line));
            data.insert("refs".into(), json!(input.refs));
            data.insert("made_by".into(), json!(answer.1));
            record(st, kind, Value::Object(data), input)?;
            pass.written.push(format!("{key} {which}"));
        }
        Err(e) => {
            failed(st, kind, key, which, &e)?;
            pass.failed.push(format!("{key} {which}"));
        }
    }
    Ok(())
}

fn record(st: &State, kind: &str, data: Value, input: &Input) -> ferb_learning::Result<()> {
    // what it was written from: the owner's words, FERB's own and its understanding; never outside content
    let mut made = input.origins.clone();
    made.insert(ferb_sanitize::origin::INFERRED.into());
    let kinds = ferb_sanitize::origin::kinds(made.iter().map(String::as_str));
    st.journal
        .append(
            kind,
            Some(data),
            Some(&ferb_sanitize::origin::join(
                kinds.iter().map(String::as_str),
            )),
        )
        .map(|_| ())
}

fn failed(
    st: &State,
    kind: &str,
    key: &str,
    which: &str,
    e: &ferb_learning::Error,
) -> ferb_learning::Result<()> {
    let why = ferb_memory::pystr::prefix(&e.described(), 500).to_owned();
    let mut data = serde_json::Map::new();
    data.insert(key.into(), json!(which));
    data.insert("error".into(), json!(why));
    st.journal
        .append(
            &format!("{kind}{FAILED_SUFFIX}"),
            Some(Value::Object(data)),
            None,
        )
        .map(|_| ())
}

/// One Librarian call: the answer and what made it (`model:<runtime>|prompt:...`).
async fn call(
    st: &Rc<State>,
    key: &str,
    input: &Input,
    schema: &Value,
) -> ferb_learning::Result<(Value, String)> {
    let make = st.make_runtime.clone().ok_or_else(|| {
        ferb_learning::Error::Runtime("no model runtime is set up for the Librarian".into())
    })?;
    let system = st
        .prompts
        .load(library::PROMPT, &[])
        .map_err(|e| ferb_learning::Error::Crash("OSError", e.to_string()))?;
    let id = format!("library-{key}-{}", input.period);
    let task =
        ferb_learning::calls::learning_task(&id, "the library's index", &st.device_id(), &id);
    let made = make(LIBRARIAN_ROLE, "supervisor", &[], schema)?;
    let spans = LearningSpans::new(st);
    let text = ferb_learning::calls::learning_call(
        &made,
        &task,
        &system,
        &input.text,
        &format!("library {key} {}", input.period),
        schema,
        LIBRARIAN_ROLE,
        Some(&spans),
        &|runtime: &str, data: &Value| spans.usage(LIBRARIAN_ROLE, runtime, data),
    )
    .await?;
    let answer = ferb_learning::calls::loads(&text)?;
    Ok((
        answer,
        format!(
            "model:{}|{}",
            made.rt.name(),
            ferb_learning::calls::prompt_tag(library::PROMPT, &system)
        ),
    ))
}
