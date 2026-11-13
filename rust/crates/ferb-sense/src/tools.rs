//! The trail tools' typed functions (`ferb/sense/tools.py`): `trail.query` and `trail.days`. They read recorded,
//! excluded and redacted activity; titles and page metadata stay outside data, never instructions, and nothing here
//! decides which observed item the owner meant. The tools' schemas and pydantic's validation text are the gateway's
//! (`rust/fixtures/catalog`); [`QueryInput::check`] holds the one check Python runs in a validator (SENSE-408).

use std::collections::BTreeSet;

use chrono::{FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};
use ferb_clock::zone::Zone;
use ferb_core::when::DateTime;
use ferb_py::time::DateTime as PyDateTime;
use serde_json::Value;

/// tools.py:15.
pub const SOURCES: [&str; 12] = [
    "app_focus",
    "window",
    "browser_tab",
    "file",
    "idle",
    "app_install",
    "workspace",
    "accessibility",
    "typed_text",
    "clipboard",
    "notification",
    "software",
];
const UNTRUSTED_SOURCES: [&str; 2] = ["browser_tab", "window"];
const PRIVATE_OUTSIDE_SOURCES: [&str; 5] = [
    "accessibility",
    "typed_text",
    "clipboard",
    "notification",
    "software",
];

/// tools.py:21 `trail_sources` (S3): window titles and pages are outside text; app focus, files, idle time and
/// installs are the owner's own.
pub fn trail_sources<S: AsRef<str>>(sources: &[S]) -> Vec<(String, String)> {
    let has = |list: &[&str]| sources.iter().any(|s| list.contains(&s.as_ref()));
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    if has(&PRIVATE_OUTSIDE_SOURCES) {
        vec![pair("web", "trail"), pair("owner_account", "trail|")]
    } else if has(&UNTRUSTED_SOURCES) {
        vec![pair("web", "trail")]
    } else {
        vec![pair("app", "trail")]
    }
}

/// Where a bound with no zone of its own is placed.
#[derive(Clone, Copy, Debug)]
pub enum Tz {
    /// A fixed offset (the daemon's clock: `datetime.now().astimezone()`).
    Fixed(FixedOffset),
    /// An IANA zone (fold=0).
    Zone(Zone),
}

fn repr(s: &str) -> String {
    ferb_sanitize::parity::repr(&ferb_sanitize::Py::str(s))
}

fn digits(s: &str) -> Option<u32> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// The text of Python's ValueError for a value `fromisoformat` refused: a field out of range when the shape was
/// read, else "Invalid isoformat string".
fn why_invalid(value: &str, date_only: bool) -> String {
    let invalid = format!("Invalid isoformat string: {}", repr(value));
    let b = value.as_bytes();
    let (date, rest) = if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' {
        (
            (value.get(0..4), value.get(5..7), value.get(8..10)),
            value.get(10..).unwrap_or(""),
        )
    } else if b.len() >= 8 && b[..8].iter().all(u8::is_ascii_digit) {
        (
            (value.get(0..4), value.get(4..6), value.get(6..8)),
            value.get(8..).unwrap_or(""),
        )
    } else {
        return invalid;
    };
    let (Some(y), Some(m), Some(d)) = (
        date.0.and_then(digits),
        date.1.and_then(digits),
        date.2.and_then(digits),
    ) else {
        return invalid;
    };
    if y == 0 {
        return format!("year {y} is out of range");
    }
    if !(1..=12).contains(&m) {
        return "month must be in 1..12".into();
    }
    if NaiveDate::from_ymd_opt(y as i32, m, d).is_none() {
        return "day is out of range for month".into();
    }
    if date_only || rest.len() < 3 {
        return invalid;
    }
    let time = &rest[1..];
    let clock = time.split(['+', '-', 'Z']).next().unwrap_or("");
    let parts: Vec<&str> = clock.split(':').collect();
    let field = |i: usize| {
        parts
            .get(i)
            .and_then(|p| digits(p.split('.').next().unwrap_or("")))
    };
    if let Some(h) = field(0) {
        if h > 23
            && !(h == 24
                && parts
                    .iter()
                    .skip(1)
                    .all(|p| p.bytes().all(|c| c == b'0' || c == b'.')))
        {
            return "hour must be in 0..23".into();
        }
    }
    if field(1).is_some_and(|m| m > 59) {
        return "minute must be in 0..59".into();
    }
    if field(2).is_some_and(|s| s > 59) {
        return "second must be in 0..59".into();
    }
    invalid
}

fn naive(p: &PyDateTime) -> Option<NaiveDateTime> {
    NaiveDate::from_ymd_opt(p.year, p.month, p.day)?.and_hms_micro_opt(
        p.hour,
        p.minute,
        p.second,
        p.microsecond,
    )
}

fn place(local: NaiveDateTime, tz: Option<Tz>, system: &Zone) -> Result<DateTime, String> {
    match tz {
        Some(Tz::Fixed(o)) => local
            .and_local_timezone(o)
            .single()
            .ok_or_else(|| "date value out of range".to_string()),
        // `replace(tzinfo=zone)`: the wall time kept, with the offset fold=0 gives it (a time in a gap keeps the
        // offset before the change, as Python writes it)
        Some(Tz::Zone(z)) => {
            let t = z.wall_dt(local).map_err(|e| e.0)?;
            let offset = (local - t.naive_utc()).num_seconds() as i32;
            let o = FixedOffset::east_opt(offset).ok_or("date value out of range")?;
            local
                .and_local_timezone(o)
                .single()
                .ok_or_else(|| "date value out of range".to_string())
        }
        None => system
            .wall_dt(local)
            .map(|t| system.local(&t))
            .map_err(|e| e.0),
    }
}

fn convert(t: &DateTime, tz: Option<Tz>, system: &Zone) -> DateTime {
    match tz {
        Some(Tz::Fixed(o)) => t.with_timezone(&o),
        Some(Tz::Zone(z)) => z.local(t),
        None => system.local(t),
    }
}

/// tools.py:29 `time_bound`: an ISO date or date-time as a bound on recorded times, in `tz` when it names none (the
/// Mac's zone, `system`, when `tz` is None), written in `tz` as the rows are. A bare date starts at its midnight; as
/// an end it covers its whole day. `Err` is Python's ValueError text.
pub fn time_bound(
    value: Option<&str>,
    end: bool,
    tz: Option<Tz>,
    system: &Zone,
) -> Result<Option<DateTime>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = ferb_py::value::strip(value);
    if value.is_empty() {
        return Ok(None);
    }
    let chars = value.chars().count();
    let parsed = PyDateTime::from_isoformat(value).map_err(|_| why_invalid(value, chars == 10))?;
    if chars == 10 {
        if parsed.hour != 0 || parsed.minute != 0 || parsed.offset.is_some() {
            return Err(why_invalid(value, true));
        }
        let day = NaiveDate::from_ymd_opt(parsed.year, parsed.month, parsed.day)
            .ok_or_else(|| why_invalid(value, true))?;
        let day = if end {
            day.succ_opt().ok_or("date value out of range")?
        } else {
            day
        };
        return place(day.and_time(NaiveTime::MIN), tz, system).map(Some);
    }
    let local = naive(&parsed).ok_or_else(|| why_invalid(value, false))?;
    match parsed.offset {
        None => place(local, tz, system).map(Some),
        Some((secs, micros)) => {
            let utc = local
                - TimeDelta::seconds(i64::from(secs))
                - TimeDelta::microseconds(i64::from(micros));
            let t = FixedOffset::east_opt(0)
                .expect("zero offset")
                .from_utc_datetime(&utc);
            Ok(Some(convert(&t, tz, system)))
        }
    }
}

use chrono::TimeZone;

/// tools.py:57 `TrailQuery.Input`.
#[derive(Clone, Debug)]
pub struct QueryInput {
    pub source: Option<String>,
    pub event_id: Option<String>,
    pub since_hours: f64,
    pub since: Option<String>,
    pub until: Option<String>,
    pub host: Option<String>,
    pub actor: Option<String>,
    pub limit: i64,
    pub before_seq: Option<i64>,
    pub text: Option<String>,
}

impl Default for QueryInput {
    fn default() -> Self {
        QueryInput {
            source: None,
            event_id: None,
            since_hours: 24.0,
            since: None,
            until: None,
            host: None,
            actor: None,
            limit: 100,
            before_seq: None,
            text: None,
        }
    }
}

impl QueryInput {
    /// tools.py:70 `_iso` (SENSE-408): a malformed since or until is the caller's error before anything is read.
    pub fn check(&self, system: &Zone) -> Result<(), String> {
        time_bound(self.since.as_deref(), false, None, system)?;
        time_bound(self.until.as_deref(), false, None, system)?;
        Ok(())
    }
}

/// tools.py:95 `TrailDays.Input`.
#[derive(Clone, Debug)]
pub struct DaysInput {
    pub text: Option<String>,
    pub since_days: i64,
    pub limit: i64,
}

impl Default for DaysInput {
    fn default() -> Self {
        DaysInput {
            text: None,
            since_days: 30,
            limit: 10,
        }
    }
}

/// memory/summary.py:33 `digest_origins`: every origin a day summary or narrative holds (MP2).
pub fn digest_origins(kind: &str, data: &Value) -> BTreeSet<String> {
    use ferb_sanitize::origin;
    let web = origin::external("web", "trail");
    if kind == crate::observation::NARRATIVE_KIND {
        return [origin::INFERRED.to_string(), web].into_iter().collect();
    }
    let get = |k: &str| data.as_object().and_then(|m| m.get(k));
    let t = |k: &str| crate::observation::truthy(get(k));
    let mut out = BTreeSet::new();
    if t("windows") || t("sites") || t("hours") {
        out.insert(web);
    }
    if t("apps_min") || t("folders") {
        out.insert(origin::PRIVATE.to_string());
    }
    if let Some(Value::Object(ferb)) = get("ferb") {
        if crate::observation::truthy(ferb.get("tasks")) {
            out.insert(origin::INFERRED.to_string());
        }
    }
    if out.is_empty() {
        out.insert(origin::PRIVATE.to_string());
    }
    out
}

/// tools.py:132 `_day_sources`.
pub fn day_sources(days: &[Value]) -> Vec<(String, String)> {
    let mut found = BTreeSet::new();
    for d in days {
        let kind = if d.get("kind") == Some(&Value::from("narrative")) {
            crate::observation::NARRATIVE_KIND
        } else {
            crate::observation::SUMMARY_KIND
        };
        found.extend(digest_origins(kind, d.get("data").unwrap_or(&Value::Null)));
    }
    found
        .into_iter()
        .map(|o| ("origin".to_string(), o))
        .collect()
}

#[cfg(feature = "store")]
pub use run::*;

#[cfg(feature = "store")]
mod run {
    use super::*;
    use crate::trail::{Query, Trail};
    use ferb_core::types::ToolResult;
    use ferb_core::when::isoformat;
    use serde_json::Map;

    fn error(text: &str) -> ToolResult {
        let mut c = Map::new();
        c.insert("error".into(), Value::from(text));
        ToolResult {
            content: c,
            is_error: true,
            ..Default::default()
        }
    }

    /// tools.py:82 `TrailQuery.run`.
    pub fn trail_query(
        trail: Option<&Trail<'_>>,
        a: &QueryInput,
    ) -> Result<ToolResult, crate::trail::Error> {
        let Some(trail) = trail else {
            return Ok(error("the trail isn't available"));
        };
        let now = trail.clock.now();
        let zone = trail.clock.zone();
        let tz = Some(Tz::Fixed(*now.offset()));
        let start =
            time_bound(a.since.as_deref(), false, tz, &zone).map_err(crate::trail::Error::Value)?;
        let end =
            time_bound(a.until.as_deref(), true, tz, &zone).map_err(crate::trail::Error::Value)?;
        let since = match start {
            Some(s) => isoformat(&s),
            None => {
                let us = ferb_clock::py::timedelta_us(a.since_hours, 3_600_000_000)
                    .map_err(|e| crate::trail::Error::Value(e.0))?;
                isoformat(
                    &ferb_clock::py::plus_us(&now, -us)
                        .map_err(|e| crate::trail::Error::Value(e.0))?,
                )
            }
        };
        let until = end.map(|e| isoformat(&e));
        let page = trail.query(&Query {
            since: &since,
            until: until.as_deref(),
            source: a.source.as_deref(),
            actor: a.actor.as_deref(),
            host: a.host.as_deref(),
            limit: a.limit,
            before_seq: a.before_seq,
            text: a.text.as_deref(),
            event_id: a.event_id.as_deref(),
        })?;
        let mut sources: Vec<String> = Vec::new();
        for e in page["events"].as_array().into_iter().flatten() {
            if let Some(Value::String(s)) = e.get("source") {
                if !sources.contains(s) {
                    sources.push(s.clone());
                }
            }
        }
        let content = match page {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        Ok(ToolResult {
            content,
            read_sources: trail_sources(&sources),
            ..Default::default()
        })
    }

    /// tools.py:117 `TrailDays.run`. `day_refs(day)`: the `kind:id` handles of that day's evidence
    /// (`memory/summary.py` `build(db, day).handles`), the memory part's.
    pub fn trail_days(
        trail: Option<&Trail<'_>>,
        a: &DaysInput,
        day_refs: &dyn Fn(&str) -> Vec<String>,
    ) -> Result<ToolResult, crate::trail::Error> {
        let Some(trail) = trail else {
            return Ok(error("the trail isn't available"));
        };
        let days = trail.days(a.text.as_deref(), a.since_days, a.limit)?;
        let mut refs: BTreeSet<String> = days
            .iter()
            .map(|d| format!("journal:{}", crate::observation::py_str(d.get("id"))))
            .collect();
        let each: BTreeSet<String> = days
            .iter()
            .map(|d| crate::observation::py_str(d.get("day")))
            .collect();
        for day in each {
            refs.extend(day_refs(&day));
        }
        let mut content = Map::new();
        content.insert("count".into(), Value::from(days.len()));
        let read_sources = day_sources(&days);
        let mut out = Map::new();
        out.insert("days".into(), Value::Array(days));
        out.insert("count".into(), content.remove("count").expect("count"));
        out.insert(
            "source_refs".into(),
            Value::Array(refs.into_iter().map(Value::from).collect()),
        );
        Ok(ToolResult {
            content: out,
            read_sources,
            ..Default::default()
        })
    }
}
