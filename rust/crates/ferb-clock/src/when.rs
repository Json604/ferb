//! Port of `ferb/clock/when.py`: when something should happen, in the owner's terms; code turns it into an absolute
//! time (owner, 2026-09-30: the model never computes dates for reminders, schedules or sleeps).
//!
//! Every function takes the zone it works in (Python's `zone or local_zone()`): the daemon passes the Mac's.

use chrono::{Datelike, NaiveDate, NaiveTime};
use serde_json::{json, Value};

use crate::py::{self, Overflow, Time};
use crate::zone::Zone;

pub const DAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// `DAY_WORDS` (`when.py:28`).
pub fn day_words() -> Vec<String> {
    let mut out: Vec<String> = ["today", "tomorrow", "day_after_tomorrow", "end_of_month"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    out.extend(DAYS.iter().map(|d| d.to_string()));
    out.extend(DAYS.iter().map(|d| format!("next_{d}")));
    out
}

/// `WHEN_SCHEMA` (`when.py:47`).
pub fn when_schema() -> Value {
    let nullable = |t: &str| json!({"anyOf": [{"type": t}, {"type": "null"}]});
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["in_minutes", "in_days", "day", "date", "time"],
        "properties": {
            "in_minutes": nullable("number"), "in_days": nullable("integer"),
            "day": {"anyOf": [{"type": "string", "enum": day_words()}, {"type": "null"}]},
            "date": nullable("string"), "time": nullable("string"),
        },
    })
}

/// `WHEN_HELP` (`when.py:54`).
pub const WHEN_HELP: &str = "when: only what the owner said — in_minutes (\"in 2 hours\" = 120), in_days, day (today / tomorrow / day_after_tomorrow / end_of_month / a weekday like \"friday\" / \"next_friday\" for next week's), date (\"MM-DD\" or \"YYYY-MM-DD\"), time (\"HH:MM\" 24-hour local); unused fields null. Never work out a date yourself: FERB computes it from the clock.";

/// The `When` model: only what the owner said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct When {
    pub in_minutes: Option<f64>,
    pub in_days: Option<i64>,
    pub day: Option<String>,
    pub date: Option<String>,
    pub time: Option<String>,
}

impl When {
    /// From a JSON object of the model's fields (missing and null are None; the gateway already checked types).
    pub fn from_json(v: &Value) -> When {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        When {
            in_minutes: v.get("in_minutes").and_then(Value::as_f64),
            in_days: v.get("in_days").and_then(Value::as_i64),
            day: s("day"),
            date: s("date"),
            time: s("time"),
        }
    }

    fn sources(&self) -> usize {
        usize::from(self.day.is_some())
            + usize::from(self.date.is_some())
            + usize::from(self.in_days.is_some())
    }
}

/// `WhenError` (a `ValueError`) with its words, or Python's `OverflowError` from date arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WhenError {
    Invalid(String),
    Overflow(String),
}

impl WhenError {
    /// Python's exception class name.
    pub fn kind(&self) -> &'static str {
        match self {
            WhenError::Invalid(_) => "WhenError",
            WhenError::Overflow(_) => "OverflowError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            WhenError::Invalid(m) | WhenError::Overflow(m) => m,
        }
    }
}

impl std::fmt::Display for WhenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<Overflow> for WhenError {
    fn from(o: Overflow) -> Self {
        WhenError::Overflow(o.0)
    }
}

fn bad(msg: impl Into<String>) -> WhenError {
    WhenError::Invalid(msg.into())
}

fn truthy(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.is_empty())
}

/// A run of `\d` from byte `at`, at most `max` long; returns the digits and where they end.
fn digits(s: &str, at: usize, min: usize, max: usize) -> Option<(&str, usize)> {
    let mut end = at;
    let mut n = 0;
    for c in s[at..].chars() {
        if n == max || !py::is_digit(c) {
            break;
        }
        end += c.len_utf8();
        n += 1;
    }
    (n >= min).then(|| (&s[at..end], end))
}

fn skip_space(s: &str, at: usize) -> usize {
    at + s[at..]
        .chars()
        .take_while(|&c| py::is_space(c))
        .map(char::len_utf8)
        .sum::<usize>()
}

/// `re.fullmatch(r"\s*(\d{1,2}):(\d{2})\s*", text)` → the two numbers.
fn clock_parts(text: &str) -> Option<(i64, i64)> {
    let at = skip_space(text, 0);
    let (h, at) = digits(text, at, 1, 2)?;
    let at = text[at..].strip_prefix(':').map(|_| at + 1)?;
    let (m, at) = digits(text, at, 2, 2)?;
    (skip_space(text, at) == text.len()).then(|| (py::int_of(h), py::int_of(m)))
}

/// `_clock` (`when.py:76`).
pub fn clock(text: &str) -> Result<NaiveTime, WhenError> {
    match clock_parts(text) {
        Some((h, m)) if h <= 23 && m <= 59 => {
            Ok(NaiveTime::from_hms_opt(h as u32, m as u32, 0).expect("checked"))
        }
        _ => Err(bad(format!(
            "time must be HH:MM (24-hour), not {}",
            py::repr(text)
        ))),
    }
}

/// `re.fullmatch(r"\s*(?:(\d{4})-)?(\d{1,2})-(\d{1,2})\s*", text)` → (year?, month, day).
fn date_parts(text: &str) -> Option<(Option<i64>, i64, i64)> {
    let start = skip_space(text, 0);
    let tail = |at: usize| -> Option<(i64, i64)> {
        let (mo, at) = digits(text, at, 1, 2)?;
        let at = text[at..].strip_prefix('-').map(|_| at + 1)?;
        let (d, at) = digits(text, at, 1, 2)?;
        (skip_space(text, at) == text.len()).then(|| (py::int_of(mo), py::int_of(d)))
    };
    // with a year first (the optional group is greedy), then without
    if let Some((y, at)) = digits(text, start, 4, 4) {
        if let Some(at) = text[at..].strip_prefix('-').map(|_| at + 1) {
            if let Some((mo, d)) = tail(at) {
                return Some((Some(py::int_of(y)), mo, d));
            }
        }
    }
    tail(start).map(|(mo, d)| (None, mo, d))
}

fn ymd(y: i64, m: i64, d: i64) -> Option<NaiveDate> {
    if !(1..=9999).contains(&y) {
        return None;
    }
    NaiveDate::from_ymd_opt(y as i32, u32::try_from(m).ok()?, u32::try_from(d).ok()?)
}

fn last_day(d: NaiveDate) -> NaiveDate {
    let (y, m) = (d.year(), d.month());
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    match next {
        Some(n) => n.pred_opt().expect("a day before"),
        None => NaiveDate::from_ymd_opt(y, 12, 31).expect("a last day"),
    }
}

/// `_day` (`when.py:91`).
fn day_of(w: &When, today: NaiveDate) -> Result<Option<NaiveDate>, WhenError> {
    if let Some(text) = w.date.as_deref().filter(|s| !s.is_empty()) {
        let Some((year, mo, dd)) = date_parts(text) else {
            return Err(bad(format!(
                "date must be MM-DD or YYYY-MM-DD, not {}",
                py::repr(text)
            )));
        };
        let y = year.unwrap_or(i64::from(today.year()));
        let none = || bad(format!("there's no {text}"));
        let mut d = ymd(y, mo, dd).ok_or_else(none)?;
        if year.is_none() && d < today {
            d = ymd(y + 1, mo, dd).ok_or_else(none)?;
        }
        return Ok(Some(d));
    }
    if let Some(n) = w.in_days {
        return Ok(Some(py::date_plus(today, n)?));
    }
    let day = w.day.as_deref().unwrap_or("").to_lowercase();
    if day.is_empty() {
        return Ok(None);
    }
    let wd = i64::from(today.weekday().num_days_from_monday());
    let plus = |n: i64| py::date_plus(today, n).map(Some).map_err(WhenError::from);
    match day.as_str() {
        "today" => return Ok(Some(today)),
        "tomorrow" => return plus(1),
        "day_after_tomorrow" => return plus(2),
        "end_of_month" => return Ok(Some(last_day(today))),
        _ => {}
    }
    if let Some(rest) = day.strip_prefix("next_") {
        if let Some(i) = DAYS.iter().position(|d| *d == rest) {
            return plus(7 - wd + i as i64);
        }
    }
    if let Some(i) = DAYS.iter().position(|d| *d == day) {
        return plus((i as i64 - wd).rem_euclid(7));
    }
    Err(bad(format!(
        "unknown day {}",
        py::repr(w.day.as_deref().unwrap_or(""))
    )))
}

fn midnight() -> NaiveTime {
    NaiveTime::MIN
}

fn local_fmt(t: &Time, zone: &Zone, pattern: &str) -> String {
    let l = zone.local(t);
    py::strftime(&l.naive_local(), l.offset().local_minus_utc(), "", pattern)
}

/// `end_of_day` (`when.py:120`): the end (next local midnight, UTC) of the calendar day `w` names, after `now`.
pub fn end_of_day(w: &When, now: &Time, zone: &Zone) -> Result<Time, WhenError> {
    if w.in_minutes.is_some() || truthy(&w.time) {
        return Err(bad("an end names a day, not a time"));
    }
    let (d, today) = end_day(w, now, zone)?;
    let next = py::date_plus(d, 1)?;
    let end = zone.wall(next, midnight())?;
    if end <= *now {
        let year = if d.year() != today.year() {
            format!(" {}", d.year())
        } else {
            String::new()
        };
        let shown = py::strftime(&d.and_time(midnight()), 0, "", "%a %d %b");
        return Err(bad(format!("that day is already over ({shown}{year})")));
    }
    Ok(end)
}

fn end_day(w: &When, now: &Time, zone: &Zone) -> Result<(NaiveDate, NaiveDate), WhenError> {
    if w.sources() != 1 {
        return Err(bad(
            "give exactly one calendar day source: day, date or in_days",
        ));
    }
    let today = zone.date(now);
    let year_first = w.date.as_deref().is_some_and(|t| {
        let at = skip_space(t, 0);
        digits(t, at, 4, 4).is_some_and(|(_, e)| t[e..].starts_with('-'))
    });
    let d = match w.date.as_deref() {
        Some(t) if !t.is_empty() && !year_first => nearest(t, today)?,
        // Python asserts here, so an empty `date` or `day` (one source, naming nothing) crashes the caller with an
        // AssertionError; fixed against Python: it is refused in the words of a missing source
        _ => day_of(w, today)?
            .ok_or_else(|| bad("give exactly one calendar day source: day, date or in_days"))?,
    };
    Ok((d, today))
}

/// `end_at` (`when.py:142`): when something the owner gave an end stops (ORCH-320), after `now`.
pub fn end_at(w: &When, now: &Time, zone: &Zone) -> Result<Time, WhenError> {
    if let Some(minutes) = w.in_minutes {
        if truthy(&w.day) || truthy(&w.date) || w.in_days.is_some() || truthy(&w.time) {
            return Err(bad("give in_minutes on its own"));
        }
        if minutes <= 0.0 {
            return Err(bad("in_minutes must be more than 0"));
        }
        let us = py::timedelta_us(minutes, py::US_PER_MINUTE)?;
        return Ok(py::plus_us(&py::utc(now), us)?);
    }
    if !truthy(&w.time) {
        return end_of_day(w, now, zone);
    }
    let (d, _) = end_day(w, now, zone)?;
    let at = zone.wall(d, clock(w.time.as_deref().unwrap_or(""))?)?;
    if at <= *now {
        return Err(bad(format!(
            "that's already past ({})",
            local_fmt(&at, zone, "%a %d %b %H:%M")
        )));
    }
    Ok(at)
}

/// `_nearest` (`when.py:163`).
fn nearest(month_day: &str, today: NaiveDate) -> Result<NaiveDate, WhenError> {
    let parts = {
        let at = skip_space(month_day, 0);
        digits(month_day, at, 1, 2).and_then(|(m, at)| {
            let at = month_day[at..].strip_prefix('-').map(|_| at + 1)?;
            let (d, at) = digits(month_day, at, 1, 2)?;
            (skip_space(month_day, at) == month_day.len()).then(|| (py::int_of(m), py::int_of(d)))
        })
    };
    let Some((m, d)) = parts else {
        return Err(bad(format!(
            "date must be MM-DD or YYYY-MM-DD, not {}",
            py::repr(month_day)
        )));
    };
    let y = i64::from(today.year());
    let found: Vec<NaiveDate> = [y - 1, y, y + 1]
        .into_iter()
        .filter_map(|year| ymd(year, m, d))
        .collect();
    found
        .into_iter()
        .min_by_key(|x| ((*x - today).num_days().abs(), *x < today))
        .ok_or_else(|| bad(format!("there's no {month_day}")))
}

/// `resolve` (`when.py:180`): the absolute time (UTC) for `w`, strictly after `now`.
pub fn resolve(w: &When, now: &Time, zone: &Zone) -> Result<Time, WhenError> {
    if w.sources() > 1 {
        return Err(bad(
            "give only one calendar day source: day, date or in_days",
        ));
    }
    let at = if let Some(minutes) = w.in_minutes {
        if minutes <= 0.0 {
            return Err(bad("in_minutes must be more than 0"));
        }
        let us = py::timedelta_us(minutes, py::US_PER_MINUTE)?;
        let at = py::plus_us(&py::utc(now), us)?;
        if truthy(&w.day) || truthy(&w.date) || w.in_days.is_some() || truthy(&w.time) {
            return Err(bad("give in_minutes on its own"));
        }
        at
    } else {
        let local_day = zone.date(now);
        let d = day_of(w, local_day)?;
        let t = match w.time.as_deref().filter(|s| !s.is_empty()) {
            Some(text) => Some(clock(text)?),
            None => None,
        };
        let Some(t) = t else {
            if d.is_none() {
                return Err(bad("say when: a time, a day, a date or how long from now"));
            }
            return Err(bad(
                "a calendar day needs an explicit local time; resolve it from owner context",
            ));
        };
        let mut at = zone.wall(d.unwrap_or(local_day), t)?;
        if at <= *now {
            let day = w.day.as_deref().unwrap_or("").to_lowercase();
            match d {
                None => at = zone.wall(py::date_plus(local_day, 1)?, t)?,
                Some(d) if DAYS.contains(&day.as_str()) => {
                    at = zone.wall(py::date_plus(d, 7)?, t)?;
                }
                Some(d) if day == "end_of_month" => {
                    let next = py::date_plus(d, 1)?;
                    at = zone.wall(last_day(next), t)?;
                }
                _ => {}
            }
        }
        at
    };
    if at <= *now {
        return Err(bad(format!(
            "that's in the past ({})",
            local_fmt(&at, zone, "%a %d %b %H:%M")
        )));
    }
    Ok(py::utc(&at))
}

/// `when_present` (`when.py:208`, ORCH-317): when something the owner wants at the Mac is first due, and the time a
/// return of theirs must come after (None: being active when it is due is enough).
pub fn when_present(
    w: &When,
    repeat: &str,
    now: &Time,
    every_minutes: Option<i64>,
    zone: &Zone,
) -> Result<(Time, Option<Time>), WhenError> {
    if w.in_minutes.is_some() || truthy(&w.time) {
        return Ok((resolve(w, now, zone)?, None));
    }
    if w.sources() > 1 {
        return Err(bad(
            "give only one calendar day source: day, date or in_days",
        ));
    }
    let local = zone.date(now);
    let d = day_of(w, local)?;
    let utc_now = py::utc(now);
    if repeat == "every" && d.is_none() {
        let every = every_minutes
            .filter(|m| *m > 0)
            .ok_or_else(|| bad("a repeating interval must be positive"))?;
        let us = py::timedelta_us(every as f64, py::US_PER_MINUTE)?;
        return Ok((py::plus_us(&utc_now, us)?, None));
    }
    let mut start = zone.wall(d.unwrap_or(local), midnight())?;
    if repeat == "none" {
        let due = if start >= utc_now { start } else { utc_now };
        return Ok((due, Some(due)));
    }
    if start <= utc_now {
        start = zone.wall(py::date_plus(local, 1)?, midnight())?;
    }
    Ok((start, None))
}
