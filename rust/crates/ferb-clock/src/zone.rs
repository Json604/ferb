//! Time zones as Python's `zoneinfo` reads them (`ferb/clock/when.py:61-73`, `_wall`), on the tz database built into
//! this crate (`chrono-tz`) instead of the operating system's, so the engine computes the same on a Mac, an iPhone
//! and an Android phone.
//!
//! The one rule that matters: a local wall time becomes an instant with PEP 495's `fold=0`, the UTC offset in force
//! *before* a change. A time skipped by a change (02:30 on a spring-forward night) moves forward by the gap (03:30);
//! a time that happens twice is the first one.

use std::str::FromStr;

use chrono::{
    FixedOffset, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeDelta, TimeZone,
};
use chrono_tz::{OffsetName, Tz};

use crate::py::{self, Overflow, Time};

/// The tz database release built in (Python reads the Mac's own, `/usr/share/zoneinfo`).
pub const TZDB_VERSION: &str = chrono_tz::IANA_TZDB_VERSION;

/// An IANA zone, as `ZoneInfo(key)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone(Tz);

impl Zone {
    /// `ZoneInfo(key)`; `None` for a key the database doesn't have.
    pub fn named(key: &str) -> Option<Zone> {
        if key.is_empty() || key.starts_with('/') || key.contains("..") {
            return None;
        }
        Tz::from_str(key).ok().map(Zone)
    }

    pub fn utc() -> Zone {
        Zone(Tz::UTC)
    }

    /// `zone.key`.
    pub fn key(&self) -> &'static str {
        self.0.name()
    }

    fn offset_at(&self, t: &Time) -> FixedOffset {
        self.0.offset_from_utc_datetime(&t.naive_utc()).fix()
    }

    /// `t.astimezone(zone)`, as a fixed-offset time (its wall time and offset).
    pub fn local(&self, t: &Time) -> Time {
        t.with_timezone(&self.offset_at(t))
    }

    /// `%Z` of `t.astimezone(zone)`: the zone's abbreviation at that instant.
    pub fn abbreviation(&self, t: &Time) -> String {
        let o = self.0.offset_from_utc_datetime(&t.naive_utc());
        match o.abbreviation() {
            Some(a) => a.to_string(),
            None => py::offset_z(o.fix().local_minus_utc()),
        }
    }

    /// `datetime.combine(day, time, tzinfo=zone)` as an instant (fold=0, see the module's note).
    pub fn wall(&self, day: NaiveDate, at: NaiveTime) -> Result<Time, Overflow> {
        self.wall_dt(day.and_time(at))
    }

    /// A naive local time in this zone as an instant, fold=0.
    pub fn wall_dt(&self, local: NaiveDateTime) -> Result<Time, Overflow> {
        let offset = match self.0.offset_from_local_datetime(&local) {
            LocalResult::Single(o) => o.fix(),
            LocalResult::Ambiguous(a, b) => {
                let (a, b) = (a.fix(), b.fix());
                if a.local_minus_utc() >= b.local_minus_utc() {
                    a
                } else {
                    b
                }
            }
            LocalResult::None => {
                // in a gap: the offset in force before it (14 h before, as an instant, is before the change)
                let probe = local - TimeDelta::hours(14);
                self.0.offset_from_utc_datetime(&probe).fix()
            }
        };
        let utc_naive = local - TimeDelta::seconds(i64::from(offset.local_minus_utc()));
        let t = FixedOffset::east_opt(0)
            .expect("zero offset")
            .from_utc_datetime(&utc_naive);
        let year = chrono::Datelike::year(&utc_naive);
        if !(1..=9999).contains(&year) {
            return Err(Overflow("date value out of range".into()));
        }
        Ok(t)
    }

    /// The local calendar day of `t` in this zone.
    pub fn date(&self, t: &Time) -> NaiveDate {
        self.local(t).date_naive()
    }
}

/// `local_zone()` (`when.py:61-73`): `TZ` from the environment, else the zone `/etc/localtime` links to. `Err` names
/// what couldn't be read. Python falls back to `ZoneInfo(time.tzname[0])` when neither gives a name; that is not
/// reproduced (the daemon passes its zone in).
pub fn local_zone() -> Result<Zone, String> {
    let name = match std::env::var("TZ") {
        Ok(v) if !v.is_empty() => Some(v),
        _ => std::fs::read_link("/etc/localtime").ok().and_then(|p| {
            p.to_string_lossy()
                .split_once("zoneinfo/")
                .map(|(_, z)| z.to_string())
        }),
    };
    let name = name.unwrap_or_default();
    Zone::named(&name).ok_or_else(|| format!("local time zone unavailable: {name}"))
}
