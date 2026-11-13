//! The calendar arithmetic of `ferb/orchestrator/schedules.py`: rules, several daily times and random windows,
//! end dates, the words code writes about a schedule's timing, the clock times a request names, and the planner's
//! fields turned into a schedule (`plan_new`, `plan_edit`). The store is in `schedule_store` (Mac-side).
//!
//! Decisions: ORCH-320 (ends), ORCH-334 (no minimum interval), ORCH-511, ORCH-550..552, ORCH-555, ORCH-556.

use std::collections::BTreeSet;

use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta};
use serde_json::{json, Map, Value};

use crate::py::{self, Time};
use crate::pyrandom::Random;
use crate::when::{self, When, WhenError};
use crate::zone::Zone;

pub const REPEATS: [&str; 5] = ["none", "daily", "weekdays", "weekly", "every"];
pub const DAYS: [&str; 7] = when::DAYS;
pub const WATCHES: [&str; 3] = ["none", "until_met", "standing"];
pub const FINISHED: [&str; 2] = ["done", "done_unverified"];
pub const RUN_SCOPE: &str = "run_scope";
pub const TIMING: &str = "timing";
pub const DAY_MINUTES: i64 = 24 * 60;

/// `WATCH_SAID`.
pub fn watch_said(watch: &str) -> &'static str {
    match watch {
        "until_met" => "until it happens, then it stops",
        "standing" => "each time it happens",
        _ => "",
    }
}

/// `ScheduleError` (a `ValueError`) or one of `when.py`'s errors on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    Invalid(String),
    When(WhenError),
}

impl ScheduleError {
    pub fn kind(&self) -> &'static str {
        match self {
            ScheduleError::Invalid(_) => "ScheduleError",
            ScheduleError::When(w) => w.kind(),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ScheduleError::Invalid(m) => m,
            ScheduleError::When(w) => w.message(),
        }
    }
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<WhenError> for ScheduleError {
    fn from(e: WhenError) -> Self {
        ScheduleError::When(e)
    }
}

impl From<py::Overflow> for ScheduleError {
    fn from(e: py::Overflow) -> Self {
        ScheduleError::When(e.into())
    }
}

fn bad(msg: impl Into<String>) -> ScheduleError {
    ScheduleError::Invalid(msg.into())
}

/// `until_said` (`schedules.py:60`): "until Mon 03 Nov" for the end of a local day, else with its time.
pub fn until_said(end: &Time, zone: &Zone) -> String {
    let local = zone.local(end);
    let wall = local.naive_local();
    let off = local.offset().local_minus_utc();
    if chrono::Timelike::num_seconds_from_midnight(&wall) == 0 {
        let before = wall - TimeDelta::seconds(1);
        return format!("until {}", py::strftime(&before, off, "", "%a %d %b"));
    }
    format!("until {}", py::strftime(&wall, off, "", "%a %d %b %H:%M"))
}

/// `scope_said` (`schedules.py:72`): what every run uses (ORCH-313). `home`: the owner's home folder; `label`: the
/// runtime catalog's name for a worker type (`ferb/runtimes/catalog.py`, a later part).
pub fn scope_said(
    scope: &Map<String, Value>,
    home: &str,
    label: &dyn Fn(&str) -> String,
) -> String {
    let get = |k: &str| scope.get(k).filter(|v| ferb_py::value::truthy(v));
    let owner = scope.get("identity").and_then(Value::as_str) == Some("owner");
    let who = if owner {
        let profile = get("profile")
            .map(|p| format!(", in your browser profile “{}”", ferb_py::value::py_str(p)))
            .unwrap_or_default();
        format!("you{profile}")
    } else {
        "FERB's own browser and accounts".to_string()
    };
    let mut parts = vec![if owner { format!("as {who}") } else { who }];
    if let Some(a) = get("account") {
        parts.push(format!("account {}", ferb_py::value::py_str(a)));
    }
    if let Some(Value::Array(folders)) = get("folders") {
        let shown: Vec<String> = folders
            .iter()
            .map(|f| {
                let f = ferb_py::value::py_str(f);
                if f == home || f.starts_with(&format!("{home}/")) {
                    format!("~{}", &f[home.len()..])
                } else {
                    f
                }
            })
            .collect();
        parts.push(format!("in {}", shown.join(", ")));
    }
    match scope.get("worker_type") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s.is_empty() || s == "default" => {}
        Some(v) => parts.push(format!("with {}", label(&ferb_py::value::py_str(v)))),
    }
    format!("Each run uses: {}.", parts.join("; "))
}

/// `watch_met` (`schedules.py:90`).
pub fn watch_met(status: &str, met: Option<bool>) -> bool {
    met == Some(true) && FINISHED.contains(&status)
}

/// `clock_minutes` (`schedules.py:105`): "HH:MM" (24-hour) as minutes after midnight.
pub fn clock_minutes(text: &str) -> Result<i64, ScheduleError> {
    clock_parse(text).ok_or_else(|| {
        bad(format!(
            "a time of day must be HH:MM (24-hour), not {}",
            py::repr(text)
        ))
    })
}

/// `CLOCK.fullmatch`: `\s*([01]?\d|2[0-3]):([0-5]\d)\s*`.
fn clock_parse(text: &str) -> Option<i64> {
    let s = text.trim_start_matches(py::is_space);
    let (hour, rest) = s.split_once(':')?;
    let hc: Vec<char> = hour.chars().collect();
    let h = match hc.as_slice() {
        [d] if py::is_digit(*d) => i64::from(py::digit_value(*d)),
        [a, d] if matches!(a, '0' | '1') && py::is_digit(*d) => {
            i64::from(py::digit_value(*a)) * 10 + i64::from(py::digit_value(*d))
        }
        ['2', d] if ('0'..='3').contains(d) => 20 + i64::from(py::digit_value(*d)),
        _ => return None,
    };
    let mut rc = rest.chars();
    let (a, b) = (rc.next()?, rc.next()?);
    if !('0'..='5').contains(&a) || !py::is_digit(b) {
        return None;
    }
    if !rc.as_str().chars().all(py::is_space) {
        return None;
    }
    Some(h * 60 + i64::from(py::digit_value(a)) * 10 + i64::from(py::digit_value(b)))
}

/// `hhmm`.
pub fn hhmm(minutes: i64) -> String {
    format!(
        "{:02}:{:02}",
        minutes.div_euclid(60),
        minutes.rem_euclid(60)
    )
}

/// `said_list`: "a", "a and b", "a, b and c".
pub fn said_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn time_of(minutes: i64) -> NaiveTime {
    NaiveTime::from_hms_opt((minutes / 60) as u32, (minutes % 60) as u32, 0)
        .expect("a minute of the day")
}

/// `Window`: `count` runs a day at random times from `start` to `end` (HH:MM, both included).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub count: i64,
    pub start: String,
    pub end: String,
}

/// `Timing` (`schedules.py:135`): fixed times and random windows beside the rule's own (T-1066).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub times: Vec<String>,
    pub windows: Vec<Window>,
    pub seed: String,
    pub start: String,
}

impl Timing {
    /// `Timing.build`: from the planner's fields. `own`: the minute its rule runs at each day (None for "every").
    pub fn build(
        times: &[String],
        windows: &[Window],
        seed: &str,
        start: NaiveDate,
        own: Option<i64>,
    ) -> Result<Option<Timing>, ScheduleError> {
        let mut fixed = BTreeSet::new();
        for t in times {
            fixed.insert(clock_minutes(t)?);
        }
        let mut made = Vec::new();
        for w in windows {
            let (count, first, last) = (w.count, clock_minutes(&w.start)?, clock_minutes(&w.end)?);
            if count <= 0 {
                return Err(bad("a number of random times must be positive"));
            }
            if last <= first {
                return Err(bad(format!(
                    "random times need a window that starts before it ends, not {}-{}",
                    hhmm(first),
                    hhmm(last)
                )));
            }
            if count > last - first + 1 {
                return Err(bad(format!(
                    "{count} random times don't fit between {} and {}",
                    hhmm(first),
                    hhmm(last)
                )));
            }
            made.push(Window {
                count,
                start: hhmm(first),
                end: hhmm(last),
            });
        }
        let beside_own = fixed.iter().any(|m| Some(*m) != own);
        if !beside_own && made.is_empty() {
            return Ok(None);
        }
        let mut kept = fixed;
        if let Some(o) = own {
            kept.insert(o);
        }
        Ok(Some(Timing {
            times: kept.into_iter().map(hhmm).collect(),
            windows: made,
            seed: seed.to_string(),
            start: start.format("%Y-%m-%d").to_string(),
        }))
    }

    /// `Timing.of`: a stored timing; None for none or one that doesn't read.
    pub fn of(data: Option<&Value>) -> Option<Timing> {
        let data = data?.as_object()?;
        let list = |k: &str| -> Option<Vec<Value>> {
            match data.get(k) {
                None | Some(Value::Null) => Some(vec![]),
                Some(v) if !ferb_py::value::truthy(v) => Some(vec![]),
                Some(Value::Array(a)) => Some(a.clone()),
                Some(Value::String(s)) => {
                    Some(s.chars().map(|c| Value::String(c.into())).collect())
                }
                Some(Value::Object(o)) => {
                    Some(o.keys().map(|k| Value::String(k.clone())).collect())
                }
                _ => None,
            }
        };
        let mut times = Vec::new();
        for t in list("times")? {
            let Value::String(s) = t else { return None };
            times.push(hhmm(clock_minutes(&s).ok()?));
        }
        let mut windows = Vec::new();
        for w in list("windows")? {
            let o = w.as_object()?;
            let count = match o.get("count")? {
                Value::Number(n) => n
                    .as_i64()
                    .or_else(|| n.as_f64().map(|f| f.trunc() as i64))?,
                Value::Bool(b) => i64::from(*b),
                Value::String(s) => py::strip(s).parse::<i64>().ok()?,
                _ => return None,
            };
            windows.push(Window {
                count,
                start: ferb_py::value::py_str(o.get("start")?),
                end: ferb_py::value::py_str(o.get("end")?),
            });
        }
        let text = |k: &str| match data.get(k) {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        };
        Some(Timing {
            times,
            windows,
            seed: text("seed"),
            start: text("start"),
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "times": self.times,
            "windows": self.windows.iter().map(|w| json!({"count": w.count, "start": w.start, "end": w.end}))
                .collect::<Vec<_>>(),
            "seed": self.seed,
            "start": self.start,
        })
    }

    /// `picks`: the random minutes of `day`, drawn from the seed and the date alone, never a `taken` minute.
    pub fn picks(&self, day: NaiveDate, taken: &BTreeSet<i64>) -> Result<Vec<i64>, ScheduleError> {
        let mut out = Vec::new();
        let mut used = taken.clone();
        for (i, w) in self.windows.iter().enumerate() {
            let (a, b) = (clock_minutes(&w.start)?, clock_minutes(&w.end)?);
            let pool: Vec<i64> = (a..=b).filter(|m| !used.contains(m)).collect();
            let k = (w.count.max(0) as usize).min(pool.len());
            let seed = format!("{}:{}:{i}", self.seed, day.format("%Y-%m-%d"));
            let chosen = Random::from_text(&seed).sample(&pool, k);
            used.extend(chosen.iter().copied());
            out.extend(chosen);
        }
        out.sort_unstable();
        Ok(out)
    }
}

/// `Rule` (`schedules.py:199`).
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub repeat: String,
    /// "HH:MM" local (daily / weekdays / weekly).
    pub at: String,
    /// 0 = Monday (weekly).
    pub weekday: Option<i64>,
    pub every_minutes: Option<i64>,
    pub tz: String,
    pub when_active: bool,
}

impl Rule {
    /// `Rule.build`.
    pub fn build(
        repeat: &str,
        first: &Time,
        weekday: Option<&str>,
        every_minutes: Option<i64>,
        zone: &Zone,
        when_active: bool,
    ) -> Result<Rule, ScheduleError> {
        if !REPEATS.contains(&repeat) {
            return Err(bad(format!("unknown repeat {}", py::repr(repeat))));
        }
        let local = zone.local(first);
        if repeat == "every" && every_minutes.is_none_or(|m| m <= 0) {
            return Err(bad("a repeating interval must be positive"));
        }
        let wd = (repeat == "weekly").then(|| {
            let named = weekday
                .filter(|w| !w.is_empty())
                .map(str::to_lowercase)
                .and_then(|w| DAYS.iter().position(|d| *d == w));
            named.map_or(i64::from(local.weekday().num_days_from_monday()), |i| {
                i as i64
            })
        });
        Ok(Rule {
            repeat: repeat.to_string(),
            at: py::fmt(&local, "%H:%M"),
            weekday: wd,
            every_minutes: if repeat == "every" {
                every_minutes
            } else {
                None
            },
            tz: zone.key().to_string(),
            when_active,
        })
    }

    /// `rule.__dict__` as stored (in field order).
    pub fn to_json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("repeat".into(), json!(self.repeat));
        m.insert("at".into(), json!(self.at));
        m.insert("weekday".into(), json!(self.weekday));
        m.insert("every_minutes".into(), json!(self.every_minutes));
        m.insert("tz".into(), json!(self.tz));
        m.insert("when_active".into(), json!(self.when_active));
        m
    }

    /// `Rule(**{k: v for k in known})` from a stored rule (keys a newer FERB added are left).
    pub fn from_json(m: &Map<String, Value>) -> Rule {
        let s = |k: &str, d: &str| m.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
        Rule {
            repeat: s("repeat", ""),
            at: s("at", ""),
            weekday: m.get("weekday").and_then(Value::as_i64),
            every_minutes: m.get("every_minutes").and_then(Value::as_i64),
            tz: s("tz", "UTC"),
            when_active: m.get("when_active").is_some_and(ferb_py::value::truthy),
        }
    }

    fn zone(&self) -> Result<Zone, ScheduleError> {
        Zone::named(&self.tz).ok_or_else(|| bad(format!("No time zone found with key {}", self.tz)))
    }

    /// `next_after`: the first run strictly after `after` (`anchor`: the schedule's first run).
    pub fn next_after(&self, after: &Time, anchor: &Time) -> Result<Option<Time>, ScheduleError> {
        if self.repeat == "none" {
            return Ok(None);
        }
        if self.repeat == "every" {
            let every = self
                .every_minutes
                .filter(|m| *m > 0)
                .ok_or_else(|| bad("a repeating interval must be positive"))?;
            if after < anchor {
                return Ok(Some(*anchor));
            }
            let step = every * py::US_PER_MINUTE;
            let gone = py::us_between(after, anchor);
            // int((after - anchor) / step): a true division of two ints, then truncated
            let n = (gone as f64 / step as f64).trunc() as i64 + 1;
            return Ok(Some(py::plus_us(anchor, n * step)?));
        }
        let zone = self.zone()?;
        let at = self.at_time()?;
        let mut day = zone.date(after);
        for _ in 0..9 {
            let cand = zone.wall(day, at)?;
            let wd = i64::from(day.weekday().num_days_from_monday());
            let ok = self.repeat == "daily"
                || (self.repeat == "weekdays" && wd < 5)
                || (self.repeat == "weekly" && Some(wd) == self.weekday);
            if ok && cand > *after {
                return Ok(Some(cand));
            }
            day = py::date_plus(day, 1)?;
        }
        Ok(None)
    }

    fn at_time(&self) -> Result<NaiveTime, ScheduleError> {
        let (h, m) = self
            .at
            .split_once(':')
            .ok_or_else(|| bad("not enough values to unpack (expected 2, got 1)"))?;
        let parse = |x: &str| {
            py::strip(x).parse::<u32>().map_err(|_| {
                bad(format!(
                    "invalid literal for int() with base 10: {}",
                    py::repr(x)
                ))
            })
        };
        NaiveTime::from_hms_opt(parse(h)?, parse(m)?, 0).ok_or_else(|| bad("hour must be in 0..23"))
    }

    /// `describe`.
    pub fn describe(&self) -> String {
        let mut s = self.rule_words();
        if self.when_active {
            s.push_str(", when you're at your Mac");
        }
        s
    }

    fn rule_words(&self) -> String {
        if self.repeat == "none" {
            return "once".into();
        }
        if self.repeat == "every" {
            return every_said(self.every_minutes.unwrap_or(0));
        }
        let at = if self.when_active && self.at == "00:00" {
            String::new()
        } else {
            format!(" at {}", self.at)
        };
        if self.repeat == "weekly" {
            return format!(
                "every {}{at}",
                capitalize(DAYS[weekday_index(self.weekday)])
            );
        }
        format!(
            "{}{at}",
            if self.repeat == "daily" {
                "every day"
            } else {
                "weekdays"
            }
        )
    }
}

/// `DAYS[weekday or 0]` (a negative index counts from the end, as Python's).
fn weekday_index(w: Option<i64>) -> usize {
    let i = w.unwrap_or(0);
    (if i < 0 { 7 + i } else { i }).clamp(0, 6) as usize
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `every_said` (`schedules.py:446`): "every 6 hours", "every 3 days and 12 hours", "every week".
pub fn every_said(minutes: i64) -> String {
    if minutes <= 0 {
        return format!("every {minutes} minutes");
    }
    if minutes % (7 * DAY_MINUTES) == 0 {
        let weeks = minutes / (7 * DAY_MINUTES);
        return if weeks == 1 {
            "every week".into()
        } else {
            format!("every {weeks} weeks")
        };
    }
    let (days, rest) = (minutes / DAY_MINUTES, minutes % DAY_MINUTES);
    let (hours, mins) = (rest / 60, rest % 60);
    let parts: Vec<String> = [(days, "day"), (hours, "hour"), (mins, "minute")]
        .into_iter()
        .filter(|(n, _)| *n != 0)
        .map(|(n, unit)| format!("{n} {unit}{}", if n == 1 { "" } else { "s" }))
        .collect();
    if parts.len() == 1 && parts[0].starts_with("1 ") {
        return format!("every {}", &parts[0][2..]);
    }
    format!("every {}", said_list(&parts))
}

/// A schedule as stored (`Schedule`, `schedules.py:261`).
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub id: String,
    pub request: String,
    pub own_words: String,
    /// Recorded from the owner's words at scheduling time (stored as intent_json).
    pub targets: Map<String, Value>,
    pub rule: Rule,
    pub next_run: Option<Time>,
    pub timer_id: Option<String>,
    pub state: String,
    pub failures: i64,
    pub first_run: Time,
    pub watch: String,
    pub last_reported_met: Option<bool>,
    pub until: Option<Time>,
    pub name: String,
}

impl Schedule {
    /// A schedule not yet saved (`_planned`'s and `plan_edit`'s probes): defaults for the stored fields.
    pub fn draft(request: &str, targets: Map<String, Value>, rule: Rule, first: Time) -> Schedule {
        Schedule {
            id: String::new(),
            request: request.to_string(),
            own_words: String::new(),
            targets,
            rule,
            next_run: None,
            timer_id: None,
            state: "active".into(),
            failures: 0,
            first_run: first,
            watch: "none".into(),
            last_reported_met: None,
            until: None,
            name: String::new(),
        }
    }

    /// `run_scope` (ORCH-313).
    pub fn run_scope(&self) -> Option<Map<String, Value>> {
        self.targets
            .get(RUN_SCOPE)
            .and_then(Value::as_object)
            .cloned()
    }

    /// `timing` (T-1066).
    pub fn timing(&self) -> Option<Timing> {
        Timing::of(self.targets.get(TIMING))
    }

    /// `describe`: its timing, its end and its watch, in plain words. `zone`: the Mac's (`until_said`).
    pub fn describe(&self, zone: &Zone) -> String {
        let parts = [
            self.when_said(),
            self.until.map(|u| until_said(&u, zone)).unwrap_or_default(),
            watch_said(&self.watch).to_string(),
        ];
        parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `when_said`: when it runs, from its saved rule and timing only.
    pub fn when_said(&self) -> String {
        let rule = &self.rule;
        let Some(timing) = self.timing() else {
            return rule.describe();
        };
        let times_or_at = || {
            if timing.times.is_empty() {
                vec![rule.at.clone()]
            } else {
                timing.times.clone()
            }
        };
        let mut said = if rule.repeat == "every" {
            let mut s = every_said(rule.every_minutes.unwrap_or(0));
            if !timing.times.is_empty() {
                s.push_str(&format!(", and at {} each day", said_list(&timing.times)));
            }
            s
        } else if rule.repeat == "none" {
            format!("once, at {}", said_list(&times_or_at()))
        } else {
            let days = match rule.repeat.as_str() {
                "daily" => "every day".to_string(),
                "weekdays" => "weekdays".to_string(),
                _ => format!("every {}", capitalize(DAYS[weekday_index(rule.weekday)])),
            };
            format!("{days} at {}", said_list(&times_or_at()))
        };
        if !timing.windows.is_empty() {
            let each = match rule.repeat.as_str() {
                "none" => "",
                "daily" | "every" => " each day",
                _ => " each of those days",
            };
            let ws: Vec<String> = timing
                .windows
                .iter()
                .map(|w| {
                    format!(
                        "{} at random times between {} and {}",
                        w.count, w.start, w.end
                    )
                })
                .collect();
            said.push_str(&format!(", plus {}{each}", said_list(&ws)));
        }
        if rule.when_active {
            said.push_str(", when you're at your Mac");
        }
        said
    }

    /// `next_after`: its first run strictly after `after`, none past its end.
    pub fn next_after(&self, after: &Time) -> Result<Option<Time>, ScheduleError> {
        let nxt = self.next(after)?;
        Ok(match (nxt, self.until) {
            (Some(n), Some(u)) if n > u => None,
            (n, _) => n,
        })
    }

    /// `ends_after`: its end, not its rule, leaves it no run after `after` (ORCH-320).
    pub fn ends_after(&self, after: &Time) -> Result<bool, ScheduleError> {
        Ok(
            self.until.is_some()
                && self.next(after)?.is_some()
                && self.next_after(after)?.is_none(),
        )
    }

    fn next(&self, after: &Time) -> Result<Option<Time>, ScheduleError> {
        let nxt = self.rule.next_after(after, &self.first_run)?;
        let extra = self.extra_after(after)?;
        Ok(match (extra, nxt) {
            (Some(e), None) => Some(e),
            (Some(e), Some(n)) if e < n => Some(e),
            (_, n) => n,
        })
    }

    fn zone(&self) -> Result<Zone, ScheduleError> {
        self.rule.zone()
    }

    /// `runs_on`: whether its rule runs on this local day.
    pub fn runs_on(&self, day: NaiveDate) -> Result<bool, ScheduleError> {
        let rule = &self.rule;
        let wd = i64::from(day.weekday().num_days_from_monday());
        Ok(matches!(rule.repeat.as_str(), "daily" | "every")
            || (rule.repeat == "weekdays" && wd < 5)
            || (rule.repeat == "weekly" && Some(wd) == rule.weekday)
            || (rule.repeat == "none" && day == self.zone()?.date(&self.first_run)))
    }

    /// `_rule_minutes`: the minutes of `day` its rule itself runs at (local).
    fn rule_minutes(&self, day: NaiveDate) -> Result<BTreeSet<i64>, ScheduleError> {
        let zone = self.zone()?;
        let rule = &self.rule;
        if rule.repeat == "every" && rule.every_minutes.is_some_and(|m| m != 0) {
            let start = zone.wall(day, NaiveTime::MIN)?;
            let end = zone.wall(py::date_plus(day, 1)?, NaiveTime::MIN)?;
            let step = rule.every_minutes.unwrap_or(0) * py::US_PER_MINUTE;
            let mut at = if start <= self.first_run {
                self.first_run
            } else {
                let gone = py::us_between(&start, &self.first_run);
                let n = -((-gone).div_euclid(step));
                py::plus_us(&self.first_run, n * step)?
            };
            let mut out = BTreeSet::new();
            if step <= 0 {
                return Ok(out);
            }
            while at < end {
                let local = zone.local(&at);
                out.insert(minute_of(&local));
                at = py::plus_us(&at, step)?;
            }
            return Ok(out);
        }
        if rule.repeat == "none" {
            let first = zone.local(&self.first_run);
            return Ok(if first.date_naive() == day {
                BTreeSet::from([minute_of(&first)])
            } else {
                BTreeSet::new()
            });
        }
        Ok(if self.runs_on(day)? {
            BTreeSet::from([clock_minutes(&rule.at)?])
        } else {
            BTreeSet::new()
        })
    }

    fn extra_minutes(&self, day: NaiveDate, timing: &Timing) -> Result<Vec<i64>, ScheduleError> {
        let mut fixed = BTreeSet::new();
        for t in &timing.times {
            fixed.insert(clock_minutes(t)?);
        }
        let mut taken = fixed.clone();
        taken.extend(self.rule_minutes(day)?);
        fixed.extend(timing.picks(day, &taken)?);
        Ok(fixed.into_iter().collect())
    }

    /// `_extra_after`.
    pub(crate) fn extra_after(&self, after: &Time) -> Result<Option<Time>, ScheduleError> {
        let Some(timing) = self.timing() else {
            return Ok(None);
        };
        let zone = self.zone()?;
        let start = if timing.start.is_empty() {
            zone.date(&self.first_run)
        } else {
            NaiveDate::parse_from_str(&timing.start, "%Y-%m-%d").map_err(|_| {
                bad(format!(
                    "Invalid isoformat string: {}",
                    py::repr(&timing.start)
                ))
            })?
        };
        let mut day = zone.date(after).max(start);
        for _ in 0..9 {
            if self.runs_on(day)? {
                for m in self.extra_minutes(day, &timing)? {
                    let at = zone.wall(day, time_of(m))?;
                    if at > *after {
                        return Ok(Some(at));
                    }
                }
            }
            day = py::date_plus(day, 1)?;
        }
        Ok(None)
    }

    /// `run_minutes`: every minute of the day it can run at, or that bounds a run (`missing_times`).
    pub fn run_minutes(&self) -> Result<BTreeSet<i64>, ScheduleError> {
        let zone = self.zone()?;
        let day = zone.date(&self.first_run);
        let mut out = BTreeSet::new();
        for i in 0..8 {
            out.extend(self.rule_minutes(py::date_plus(day, i)?)?);
        }
        if self.rule.repeat != "every" && self.rule.repeat != "none" {
            out.insert(clock_minutes(&self.rule.at)?);
        }
        if let Some(timing) = self.timing() {
            for t in &timing.times {
                out.insert(clock_minutes(t)?);
            }
            for w in &timing.windows {
                out.insert(clock_minutes(&w.start)?);
                out.insert(clock_minutes(&w.end)?);
            }
        }
        if let Some(u) = self.until {
            out.insert(minute_of(&zone.local(&u)));
        }
        Ok(out)
    }
}

fn minute_of(t: &Time) -> i64 {
    let l = t.naive_local();
    i64::from(chrono::Timelike::hour(&l)) * 60 + i64::from(chrono::Timelike::minute(&l))
}

// ---------- the clock times a request names (ORCH-551) ----------

fn is_word(c: char) -> bool {
    py::is_word(c)
}

/// One `_CLOCK_SAID` match: its text and, for the 12-hour form, its digits, minutes and a/p.
struct Said {
    start: usize,
    end: usize,
    twelve: Option<(i64, Option<i64>, char)>,
    hm: Option<(i64, i64)>,
}

/// `\d{1,2}` greedy at `at`, with its alternatives (longest first).
fn digit_runs(s: &str, at: usize) -> Vec<(i64, usize)> {
    let mut out = Vec::new();
    let mut end = at;
    let mut text = String::new();
    for c in s[at..].chars().take(2) {
        if !py::is_digit(c) {
            break;
        }
        end += c.len_utf8();
        text.push(c);
        out.push((py::int_of(&text), end));
    }
    out.reverse();
    out
}

fn ascii_ci(c: Option<char>, want: char) -> bool {
    c.is_some_and(|c| c.eq_ignore_ascii_case(&want))
}

/// The 12-hour alternative at `at`: `(\d{1,2})(?:[:.](\d{2}))?\s*([ap])(?:\.\s*m\.|\s*m\b)`.
fn match_twelve(s: &str, at: usize) -> Option<Said> {
    for (hour, after_h) in digit_runs(s, at) {
        // the minutes group is tried first (greedy ?), then without it
        let mut options: Vec<(Option<i64>, usize)> = Vec::new();
        let mut it = s[after_h..].chars();
        if let Some(sep) = it.next().filter(|c| matches!(c, ':' | '.')) {
            let a = it.next().filter(|c| py::is_digit(*c));
            let b = it.next().filter(|c| py::is_digit(*c));
            if let (Some(a), Some(b)) = (a, b) {
                let end = after_h + sep.len_utf8() + a.len_utf8() + b.len_utf8();
                options.push((Some(py::int_of(&format!("{a}{b}"))), end));
            }
        }
        options.push((None, after_h));
        for (minute, pos) in options {
            let pos = pos
                + s[pos..]
                    .chars()
                    .take_while(|c| py::is_space(*c))
                    .map(char::len_utf8)
                    .sum::<usize>();
            let mut it = s[pos..].chars();
            let Some(ap) = it
                .next()
                .filter(|c| matches!(c.to_ascii_lowercase(), 'a' | 'p'))
            else {
                continue;
            };
            let after_ap = pos + 1;
            // (?:\.\s*m\.|\s*m\b)
            let rest = &s[after_ap..];
            if let Some(r) = rest.strip_prefix('.') {
                let sp: usize = r
                    .chars()
                    .take_while(|c| py::is_space(*c))
                    .map(char::len_utf8)
                    .sum();
                let r2 = &r[sp..];
                let mut ci = r2.chars();
                if ascii_ci(ci.next(), 'm') && ci.next() == Some('.') {
                    let end = after_ap + 1 + sp + 2;
                    return Some(Said {
                        start: at,
                        end,
                        twelve: Some((hour, minute, ap.to_ascii_lowercase())),
                        hm: None,
                    });
                }
            }
            let sp: usize = rest
                .chars()
                .take_while(|c| py::is_space(*c))
                .map(char::len_utf8)
                .sum();
            let r2 = &rest[sp..];
            let mut ci = r2.chars();
            if ascii_ci(ci.next(), 'm') {
                let end = after_ap + sp + 1;
                let next = s[end..].chars().next();
                if !next.is_some_and(is_word) {
                    return Some(Said {
                        start: at,
                        end,
                        twelve: Some((hour, minute, ap.to_ascii_lowercase())),
                        hm: None,
                    });
                }
            }
        }
    }
    None
}

/// The 24-hour alternative at `at`: `([01]?\d|2[0-3]):([0-5]\d)(?![\d:])`.
fn match_hm(s: &str, at: usize) -> Option<Said> {
    let chars: Vec<(usize, char)> = s[at..].char_indices().take(6).collect();
    let c = |i: usize| chars.get(i).map(|&(_, c)| c);
    let pos = |i: usize| at + chars.get(i).map_or(s.len() - at, |&(o, _)| o);
    // hour options in the pattern's order: [01]\d, \d, 2[0-3]
    let mut hours: Vec<(i64, usize)> = Vec::new();
    if let (Some(a), Some(d)) = (c(0), c(1)) {
        if matches!(a, '0' | '1') && py::is_digit(d) {
            hours.push((
                i64::from(py::digit_value(a)) * 10 + i64::from(py::digit_value(d)),
                2,
            ));
        }
    }
    if let Some(d) = c(0).filter(|d| py::is_digit(*d)) {
        hours.push((i64::from(py::digit_value(d)), 1));
    }
    if let (Some('2'), Some(d)) = (c(0), c(1)) {
        if ('0'..='3').contains(&d) {
            hours.push((20 + i64::from(py::digit_value(d)), 2));
        }
    }
    for (hour, n) in hours {
        if c(n) != Some(':') {
            continue;
        }
        let (Some(a), Some(b)) = (c(n + 1), c(n + 2)) else {
            continue;
        };
        if !('0'..='5').contains(&a) || !py::is_digit(b) {
            continue;
        }
        if c(n + 3).is_some_and(|x| py::is_digit(x) || x == ':') {
            continue;
        }
        let minute = i64::from(py::digit_value(a)) * 10 + i64::from(py::digit_value(b));
        return Some(Said {
            start: at,
            end: pos(n + 3),
            twelve: None,
            hm: Some((hour, minute)),
        });
    }
    None
}

/// `_CLOCK_SAID.finditer(text)` (`schedules.py:408`, its lookbehinds hand-written).
fn clock_said(text: &str) -> Vec<Said> {
    let mut out = Vec::new();
    let mut at = 0;
    while at <= text.len() {
        let before = text[..at].chars().next_back();
        let free = !before.is_some_and(|c| is_word(c) || c == ':' || c == '.');
        let found = if free {
            match_twelve(text, at).or_else(|| match_hm(text, at))
        } else {
            None
        };
        match found {
            Some(m) => {
                at = if m.end > m.start { m.end } else { m.end + 1 };
                out.push(m);
            }
            None => match text[at..].chars().next() {
                Some(c) => at += c.len_utf8(),
                None => break,
            },
        }
    }
    out
}

/// `said_times`: each clock time written in `text`, with the minutes of the day it can be.
pub fn said_times(text: &str) -> Vec<(String, BTreeSet<i64>)> {
    let mut out = Vec::new();
    for m in clock_said(text) {
        let whole = &text[m.start..m.end];
        if let Some((hour, minute, ap)) = m.twelve {
            let minute = minute.unwrap_or(0);
            if !(1..=12).contains(&hour) || minute > 59 {
                continue;
            }
            let h = hour % 12 + if ap == 'p' { 12 } else { 0 };
            out.push((
                py::strip(whole).to_string(),
                BTreeSet::from([h * 60 + minute]),
            ));
        } else if let Some((hour, minute)) = m.hm {
            let mut can = BTreeSet::from([hour * 60 + minute]);
            if (1..=11).contains(&hour) {
                can.insert((hour + 12) * 60 + minute);
            }
            out.push((whole.to_string(), can));
        }
    }
    out
}

/// `missing_times`: the clock times `request` names that its schedule never runs at.
pub fn missing_times(request: &str, runs: &BTreeSet<i64>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (said, can) in said_times(request) {
        if can.is_disjoint(runs) && !out.contains(&said) {
            out.push(said);
        }
    }
    out
}

// ---------- the planner's fields → a schedule ----------

/// A schedule as code worked it out (`Plan`, `schedules.py:434`).
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub rule: Rule,
    /// Its rule's first run (the anchor of "every N minutes").
    pub first: Time,
    /// Its first run: the earlier of that and its timing's first time.
    pub booked: Time,
    pub return_after: Option<Time>,
    pub until: Option<Time>,
    pub timing: Option<Timing>,
    pub schedule: Schedule,
    pub missing: Vec<String>,
}

/// The planner's schedule fields (`plan_new`'s `spec`).
#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub request: String,
    pub when: When,
    pub repeat: String,
    pub weekday: Option<String>,
    pub every_minutes: Option<i64>,
    pub times: Vec<String>,
    pub random: Vec<Window>,
    pub when_active: bool,
    /// Its end's `When` (ORCH-320), already quote-checked.
    pub until: Option<When>,
}

/// A schedule change's timing (`plan_edit`'s `timing_spec`); `repeat` "" keeps the timing.
#[derive(Clone, Debug, Default)]
pub struct TimingSpec {
    pub repeat: String,
    pub times: Vec<String>,
    pub every_minutes: i64,
    pub weekday: String,
    pub random: Vec<Window>,
}

fn own_minute(rule: &Rule) -> Result<Option<i64>, ScheduleError> {
    if rule.repeat == "every" {
        Ok(None)
    } else {
        clock_minutes(&rule.at).map(Some)
    }
}

/// `_on_sequence`: `times` without those "every N minutes" from its first run already makes every day.
fn on_sequence(rule: &Rule, first: &Time, times: &[String]) -> Result<Vec<String>, ScheduleError> {
    let n = rule.every_minutes.unwrap_or(0);
    if rule.repeat != "every" || n == 0 || DAY_MINUTES % n != 0 {
        return Ok(times.to_vec());
    }
    let local = rule.zone()?.local(first);
    let anchor = minute_of(&local);
    let mut out = Vec::new();
    for t in times {
        if (clock_minutes(t)? - anchor).rem_euclid(n) != 0 {
            out.push(t.clone());
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn planned(
    request: &str,
    rule: Rule,
    first: Time,
    until: Option<Time>,
    timing: Option<Timing>,
    now: &Time,
    old: Option<&Schedule>,
    return_after: Option<Time>,
) -> Result<Plan, ScheduleError> {
    let mut targets: Map<String, Value> = old
        .map(|o| {
            o.targets
                .iter()
                .filter(|(k, _)| k.as_str() != TIMING)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(t) = &timing {
        targets.insert(TIMING.into(), t.to_json());
    }
    let mut s = Schedule::draft(request, targets, rule.clone(), first);
    if let Some(o) = old {
        s.id = o.id.clone();
        s.own_words = o.own_words.clone();
        s.state = o.state.clone();
        s.watch = o.watch.clone();
        s.name = o.name.clone();
    }
    s.until = until;
    let extra = s.extra_after(now)?;
    let booked = match extra {
        Some(e) if e < first => e,
        _ => first,
    };
    let missing = missing_times(request, &s.run_minutes()?);
    Ok(Plan {
        rule,
        first,
        booked,
        return_after,
        until,
        timing,
        schedule: s,
        missing,
    })
}

/// `plan_new` (`schedules.py:488`): a new schedule from the planner's fields; code computes its first run, its end
/// and its timing. `zone`: the Mac's (Python's `local_zone()`).
pub fn plan_new(spec: &Spec, now: &Time, seed: &str, zone: &Zone) -> Result<Plan, ScheduleError> {
    let present = spec.when_active;
    let mut return_after = None;
    let mut first = if present {
        let (f, r) = when::when_present(&spec.when, &spec.repeat, now, spec.every_minutes, zone)?;
        return_after = r;
        f
    } else {
        when::resolve(&spec.when, now, zone)?
    };
    let until = match &spec.until {
        Some(w) => Some(when::end_at(w, now, zone)?),
        None => None,
    };
    let rule = Rule::build(
        &spec.repeat,
        &first,
        spec.weekday.as_deref(),
        spec.every_minutes,
        zone,
        present,
    )?;
    if present
        && matches!(spec.repeat.as_str(), "daily" | "weekdays" | "weekly")
        && spec.when.time.is_none()
    {
        let probe = py::plus_us(&first, -py::US_PER_SECOND)?;
        if let Some(n) = rule.next_after(&probe, &first)? {
            first = n;
        }
    }
    if first <= *now {
        first = py::plus_us(now, py::US_PER_SECOND)?;
    }
    let rz = rule.zone()?;
    let named = spec.when.day.is_some() || spec.when.date.is_some() || spec.when.in_days.is_some();
    let start = rz.date(if named { &first } else { now });
    let timing = Timing::build(
        &on_sequence(&rule, &first, &spec.times)?,
        &spec.random,
        seed,
        start,
        own_minute(&rule)?,
    )?;
    planned(
        &spec.request,
        rule,
        first,
        until,
        timing,
        now,
        None,
        return_after,
    )
}

/// `plan_edit` (`schedules.py:515`): a schedule changed in place (T-1066).
pub fn plan_edit(
    old: &Schedule,
    request: &str,
    timing: &TimingSpec,
    now: &Time,
    seed: &str,
) -> Result<Plan, ScheduleError> {
    let words = if py::strip(request).is_empty() {
        old.request.clone()
    } else {
        request.to_string()
    };
    if timing.repeat.is_empty() {
        return planned(
            &words,
            old.rule.clone(),
            old.first_run,
            old.until,
            old.timing(),
            now,
            Some(old),
            None,
        );
    }
    let zone = old.rule.zone()?;
    let local_now = zone.local(now);
    let mut times = Vec::new();
    for t in &timing.times {
        times.push(hhmm(clock_minutes(t)?));
    }
    // `next_at`: at > local_now compares wall times (the same tzinfo in Python), then +1 day as a wall time
    let next_at = |t: &str| -> Result<Time, ScheduleError> {
        let m = clock_minutes(t)?;
        let day = local_now.date_naive();
        let at_wall = day.and_time(time_of(m));
        let day = if at_wall > local_now.naive_local() {
            day
        } else {
            py::date_plus(day, 1)?
        };
        Ok(zone.wall(day, time_of(m))?)
    };
    let every = timing.every_minutes;
    let weekday = Some(timing.weekday.as_str()).filter(|w| !w.is_empty());
    let (first, rule) = if timing.repeat == "every" {
        let first = match times.first() {
            Some(t) => next_at(t)?,
            None => py::plus_us(now, py::timedelta_us(every as f64, py::US_PER_MINUTE)?)?,
        };
        let rule = Rule::build(
            "every",
            &first,
            None,
            Some(every).filter(|e| *e != 0),
            &zone,
            old.rule.when_active,
        )?;
        (first, rule)
    } else {
        if times.is_empty() {
            return Err(bad("say the time of day it runs"));
        }
        let mut soonest: Option<Time> = None;
        for t in &times {
            let a = next_at(t)?;
            soonest = Some(soonest.map_or(a, |s| if a < s { a } else { s }));
        }
        let soonest = soonest.expect("times");
        let probe_rule = Rule::build(&timing.repeat, &soonest, weekday, None, &zone, false)?;
        let probe = Schedule::draft(&words, Map::new(), probe_rule, soonest);
        let mut first: Option<Time> = None;
        for i in 0..8 {
            let day = py::date_plus(local_now.date_naive(), i)?;
            if timing.repeat == "none" || probe.runs_on(day)? {
                for t in &times {
                    let at = zone.wall(day, time_of(clock_minutes(t)?))?;
                    if at > *now {
                        first = Some(first.map_or(at, |f| if at < f { at } else { f }));
                    }
                }
            }
        }
        let first = first.ok_or_else(|| bad("min() arg is an empty sequence"))?;
        let rule = Rule::build(
            &timing.repeat,
            &first,
            weekday,
            None,
            &zone,
            old.rule.when_active,
        )?;
        (first, rule)
    };
    let old_seed = old.timing().map(|t| t.seed).filter(|s| !s.is_empty());
    let timing = Timing::build(
        &on_sequence(&rule, &first, &times)?,
        &timing.random,
        old_seed.as_deref().unwrap_or(seed),
        local_now.date_naive(),
        own_minute(&rule)?,
    )?;
    planned(&words, rule, first, old.until, timing, now, Some(old), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_said_reads_as_a_person_says_it() {
        assert_eq!(every_said(5040), "every 3 days and 12 hours");
        assert_eq!(every_said(1500), "every 1 day and 1 hour");
        assert_eq!(every_said(60), "every hour");
        assert_eq!(every_said(10080), "every week");
    }
}
