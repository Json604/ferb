//! Port of `ferb/clock/context.py`: the time facts attached to model turns and tool results.
//!
//! `now` is what the clock gives (`datetime.now().astimezone()`: a fixed offset); `now_name` is its `%Z` (the zone's
//! abbreviation, or Python's `UTC` / `UTC+05:30` for an unnamed fixed offset: [`crate::py::fixed_tzname`]). Every other
//! time is shown in its own offset, as Python shows it.

use crate::py::{self, Time};

/// `_span` (`context.py:8`).
pub fn span(seconds: f64) -> String {
    let s = if seconds.is_nan() {
        0
    } else {
        seconds.max(0.0).trunc() as i64
    };
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{:02}m", s / 3600, s % 3600 / 60)
    }
}

fn zone_text(now: &Time, now_name: &str) -> String {
    if now_name.is_empty() {
        py::offset_z(now.offset().local_minus_utc())
    } else {
        now_name.to_string()
    }
}

/// `header` (`context.py:18`): for the start of every model turn.
pub fn header(
    now: &Time,
    now_name: &str,
    task_started: Option<&Time>,
    deadline: Option<&Time>,
    owner_last_active: Option<&Time>,
    last_trail_event: Option<&Time>,
) -> String {
    let mut parts = vec![format!(
        "now {} {}",
        py::fmt(now, "%A %Y-%m-%d %H:%M:%S"),
        zone_text(now, now_name)
    )];
    if let Some(t) = task_started {
        parts.push(format!(
            "task started {} (+{})",
            py::fmt(t, "%H:%M:%S"),
            span(py::total_seconds(now, t))
        ));
    }
    if let Some(d) = deadline {
        let remaining = py::total_seconds(d, now);
        let when = if remaining >= 0.0 {
            format!("in {}", span(remaining))
        } else {
            format!("passed {} ago", span(-remaining))
        };
        parts.push(format!("deadline {} ({when})", py::fmt(d, "%H:%M:%S")));
    }
    parts.push(format!(
        "owner last active {}",
        owner_last_active.map_or("unknown".to_string(), |a| format!(
            "{} ago",
            span(py::total_seconds(now, a))
        ))
    ));
    parts.push(format!(
        "last trail event {}",
        last_trail_event.map_or("unknown".to_string(), |a| format!(
            "{} ago",
            span(py::total_seconds(now, a))
        ))
    ));
    format!("[{}]", parts.join(" | "))
}

/// `footer` (`context.py:36`): appended to every tool result.
pub fn footer(
    now: &Time,
    now_name: &str,
    task_started: Option<&Time>,
    deadline: Option<&Time>,
) -> String {
    let mut parts = vec![format!(
        "now {} {}",
        py::fmt(now, "%H:%M:%S"),
        zone_text(now, now_name)
    )];
    if let Some(t) = task_started {
        parts.push(format!("task +{}", span(py::total_seconds(now, t))));
    }
    if let Some(d) = deadline {
        let remaining = py::total_seconds(d, now);
        parts.push(if remaining >= 0.0 {
            format!("deadline in {}", span(remaining))
        } else {
            format!("deadline passed {} ago", span(-remaining))
        });
    }
    format!("[{}]", parts.join(" | "))
}

/// `ago` (`context.py:50`): a relative age with the absolute time, `then` shown in `now`'s offset.
pub fn ago(then: &Time, now: &Time) -> String {
    let secs = py::total_seconds(now, then);
    let local = then.with_timezone(now.offset());
    let stamp = py::fmt(&local, "%a %d %b, %H:%M");
    if secs < 0.0 {
        return format!("in {} ({stamp})", span(-secs));
    }
    let rel = if secs < 60.0 {
        "just now".to_string()
    } else if secs < 3600.0 {
        format!("{} min ago", (secs / 60.0).floor() as i64)
    } else if secs < 86400.0 {
        format!("{} h ago", (secs / 3600.0).floor() as i64)
    } else if secs < 2.0 * 86400.0 {
        "yesterday".to_string()
    } else {
        format!("{} days ago", (secs / 86400.0).floor() as i64)
    };
    format!("{rel} ({stamp})")
}

/// `time_of_day` (`context.py:70`).
pub fn time_of_day(now: &Time) -> &'static str {
    let h = chrono::Timelike::hour(&now.naive_local());
    match h {
        0..=4 => "night",
        5..=11 => "morning",
        12..=16 => "afternoon",
        17..=21 => "evening",
        _ => "night",
    }
}

/// What [`time_block`] is told besides `now`.
#[derive(Clone, Debug, Default)]
pub struct Facts<'a> {
    pub task_started: Option<&'a Time>,
    pub deadline: Option<&'a Time>,
    pub owner_last_active: Option<&'a Time>,
    pub last_trail_event: Option<&'a Time>,
    /// Tagged items and when each was made, in order.
    pub ages: &'a [(String, Time)],
    pub since_last_talk: Option<&'a [String]>,
}

/// `time_block` (`context.py:75`): everything time-dependent, for the end of a prompt.
pub fn time_block(now: &Time, now_name: &str, f: &Facts<'_>) -> String {
    let mut lines = vec![
        header(
            now,
            now_name,
            f.task_started,
            f.deadline,
            f.owner_last_active,
            f.last_trail_event,
        ),
        format!("Time of day: {}.", time_of_day(now)),
    ];
    if let Some(a) = f.owner_last_active {
        let away = py::total_seconds(now, a);
        if away >= 300.0 {
            lines.push(format!(
                "The owner has been away from the Mac for {}.",
                span(away)
            ));
        }
    }
    if !f.ages.is_empty() {
        let items: Vec<String> = f
            .ages
            .iter()
            .map(|(k, v)| format!("[{k}] {}", ago(v, now)))
            .collect();
        lines.push(format!("Ages of the items above: {}", items.join("; ")));
    }
    if let Some(since) = f.since_last_talk.filter(|s| !s.is_empty()) {
        let items: Vec<String> = since.iter().map(|x| format!("- {x}")).collect();
        lines.push(format!(
            "Since you last talked with the owner:\n{}",
            items.join("\n")
        ));
    }
    lines.join("\n")
}
