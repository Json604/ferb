//! Python's `datetime` arithmetic the memory code uses, on `ferb_py::time::DateTime`: comparison (aware times by
//! their instant; a naive with an aware one raises `TypeError` in Python), subtraction, `timedelta` added,
//! `fromtimestamp`, `astimezone()` to this device's zone (CPython's `local_to_seconds`, fold 0) and
//! `timedelta.total_seconds()`'s correctly rounded division.

use std::cmp::Ordering;

pub use ferb_py::time::{DateTime, Timespec};

use crate::pyval::Crash;

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub(crate) fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 0001-01-01 (day 0).
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - 1;
    let before: i64 = (1..month).map(|m| i64::from(days_in_month(year, m))).sum();
    y * 365 + y / 4 - y / 100 + y / 400 + before + i64::from(day) - 1
}

pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let mut n = days;
    let n400 = n.div_euclid(146_097);
    n = n.rem_euclid(146_097);
    let n100 = (n / 36_524).min(3);
    n -= n100 * 36_524;
    let n4 = n / 1461;
    n -= n4 * 1461;
    let n1 = (n / 365).min(3);
    n -= n1 * 365;
    let year = n400 * 400 + n100 * 100 + n4 * 4 + n1 + 1;
    let mut month = 1;
    while n >= i64::from(days_in_month(year, month)) {
        n -= i64::from(days_in_month(year, month));
        month += 1;
    }
    (year, month, n as u32 + 1)
}

const UNIX_DAYS: i64 = 719_162; // days from 0001-01-01 to 1970-01-01

/// Microseconds of the wall time shown, from 1970-01-01T00:00 of the same wall clock.
pub fn wall_micros(dt: &DateTime) -> i64 {
    let days = days_from_civil(i64::from(dt.year), dt.month, dt.day) - UNIX_DAYS;
    ((days * 86_400 + i64::from(dt.hour) * 3600 + i64::from(dt.minute) * 60 + i64::from(dt.second))
        * 1_000_000)
        + i64::from(dt.microsecond)
}

fn offset_micros(dt: &DateTime) -> i64 {
    dt.offset
        .map_or(0, |(s, us)| i64::from(s) * 1_000_000 + i64::from(us))
}

/// Unix microseconds of an aware time (a naive one: of its wall time read as UTC).
pub fn instant(dt: &DateTime) -> i64 {
    wall_micros(dt) - offset_micros(dt)
}

/// A wall time from unix-epoch microseconds of the wall clock, with `offset`; `None` outside years 1..9999.
pub fn from_wall_micros(micros: i64, offset: Option<(i32, i32)>) -> Option<DateTime> {
    let days = micros.div_euclid(86_400_000_000);
    let rest = micros.rem_euclid(86_400_000_000);
    let (year, month, day) = civil_from_days(days + UNIX_DAYS);
    if !(1..=9999).contains(&year) {
        return None;
    }
    let seconds = rest / 1_000_000;
    Some(DateTime {
        year: year as i32,
        month,
        day,
        hour: (seconds / 3600) as u32,
        minute: (seconds / 60 % 60) as u32,
        second: (seconds % 60) as u32,
        microsecond: (rest % 1_000_000) as u32,
        offset,
    })
}

/// `a < b` and friends: aware times by instant, naive ones by wall time; mixed raises `TypeError`.
pub fn compare(a: &DateTime, b: &DateTime) -> Result<Ordering, Crash> {
    match (a.offset.is_some(), b.offset.is_some()) {
        (true, true) | (false, false) => Ok(instant(a).cmp(&instant(b))),
        _ => Err(Crash("TypeError")),
    }
}

/// `a == b`: a naive and an aware time are never equal (Python answers False, no error).
pub fn equal(a: &DateTime, b: &DateTime) -> bool {
    a.offset.is_some() == b.offset.is_some() && instant(a) == instant(b)
}

/// `(a - b)` in microseconds.
pub fn minus(a: &DateTime, b: &DateTime) -> Result<i64, Crash> {
    match (a.offset.is_some(), b.offset.is_some()) {
        (true, true) | (false, false) => Ok(instant(a) - instant(b)),
        _ => Err(Crash("TypeError")),
    }
}

/// `dt + timedelta(microseconds=micros)`, the offset kept; `None` past the calendar's ends (OverflowError).
pub fn plus(dt: &DateTime, micros: i64) -> Option<DateTime> {
    from_wall_micros(wall_micros(dt).checked_add(micros)?, dt.offset)
}

/// `n / d` for integers, correctly rounded as Python's true division of ints is.
pub fn exact_div(n: i64, d: i64) -> f64 {
    let (n_abs, d_abs) = (u128::from(n.unsigned_abs()), u128::from(d.unsigned_abs()));
    let negative = (n < 0) != (d < 0);
    if n == 0 {
        return 0.0;
    }
    let value = if n_abs < (1 << 53) && d_abs < (1 << 53) {
        n_abs as f64 / d_abs as f64
    } else {
        // the quotient scaled by 2^64, its remainder, then rounded half to even at 53 bits
        let scaled = n_abs << 64;
        let (q, r) = (scaled / d_abs, scaled % d_abs);
        let bits = 128 - q.leading_zeros() as i32;
        let drop = bits - 53;
        if drop <= 0 {
            q as f64 * 2f64.powi(-64)
        } else {
            let mut mant = q >> drop;
            let rest = q & ((1u128 << drop) - 1);
            let half = 1u128 << (drop - 1);
            if rest > half || (rest == half && (r != 0 || mant & 1 == 1)) {
                mant += 1;
            }
            mant as f64 * 2f64.powi(drop - 64)
        }
    };
    if negative {
        -value
    } else {
        value
    }
}

/// `timedelta.total_seconds()` of a span in microseconds.
pub fn total_seconds(micros: i64) -> f64 {
    exact_div(micros, 1_000_000)
}

/// `datetime.fromtimestamp(t, tz=timezone.utc)` for a float `t` (microseconds rounded half to even); `None`
/// where Python raises (NaN, infinite, outside years 1..9999).
pub fn from_timestamp_utc(t: f64) -> Option<DateTime> {
    if !t.is_finite() {
        return None;
    }
    let whole = t.trunc();
    let mut frac = ((t - whole) * 1e6).round_ties_even();
    let mut secs = whole;
    if frac >= 1e6 {
        frac -= 1e6;
        secs += 1.0;
    } else if frac < 0.0 {
        frac += 1e6;
        secs -= 1.0;
    }
    if !(-62_135_596_800.0..=253_402_300_799.0).contains(&secs) {
        return None;
    }
    from_wall_micros(secs as i64 * 1_000_000 + frac as i64, Some((0, 0)))
}

/// This device's zone, as `time.localtime` reads it: the UTC offset in force at a unix second.
pub trait LocalZone: Send + Sync {
    fn offset_at(&self, unix_seconds: i64) -> i32;
}

/// A zone with one offset for all time (tests; the fixtures use TZ="FX-05:30").
#[derive(Clone, Copy, Debug)]
pub struct FixedZone(pub i32);

impl LocalZone for FixedZone {
    fn offset_at(&self, _unix_seconds: i64) -> i32 {
        self.0
    }
}

/// The device's own zone (`localtime_r`).
#[cfg(unix)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemZone;

#[cfg(unix)]
impl LocalZone for SystemZone {
    fn offset_at(&self, unix_seconds: i64) -> i32 {
        let t = unix_seconds as libc::time_t;
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        // SAFETY: `t` is a valid time_t and `tm` points to writable memory of the right size; localtime_r fills
        // it when it returns non-null, and only then is it read.
        unsafe {
            if libc::localtime_r(&t, tm.as_mut_ptr()).is_null() {
                return 0;
            }
            tm.assume_init().tm_gmtoff as i32
        }
    }
}

fn floor_seconds(micros: i64) -> i64 {
    micros.div_euclid(1_000_000)
}

/// `dt.astimezone()` with no argument: to this device's zone. A naive `dt` is read as local wall time first
/// (CPython's `local_to_seconds`, fold 0).
pub fn astimezone(dt: &DateTime, zone: &dyn LocalZone) -> Option<DateTime> {
    let utc = if dt.offset.is_some() {
        instant(dt)
    } else {
        let wall = wall_micros(dt);
        let t = floor_seconds(wall);
        let local = |u: i64| u + i64::from(zone.offset_at(u));
        let a = local(t) - t;
        let u1 = t - a;
        let t1 = local(u1);
        let mut chosen = None;
        let b = if t1 == t {
            let u2 = u1 - 24 * 3600;
            let bb = local(u2) - u2;
            if a == bb {
                chosen = Some(u1);
            }
            bb
        } else {
            t1 - u1
        };
        let seconds = chosen.unwrap_or_else(|| {
            let u2 = t - b;
            if local(u2) == t {
                u2
            } else if t1 == t {
                u1
            } else {
                u1.max(u2)
            }
        });
        let first = i64::from(zone.offset_at(seconds)) * 1_000_000;
        wall - first
    };
    let offset = zone.offset_at(floor_seconds(utc));
    from_wall_micros(utc + i64::from(offset) * 1_000_000, Some((offset, 0)))
}

/// `dt.strftime("%Y-%m-%d")` / `date.isoformat()`.
pub fn ymd(dt: &DateTime) -> String {
    format!("{:04}-{:02}-{:02}", dt.year, dt.month, dt.day)
}

/// `datetime.fromisoformat(text)`.
pub fn parse_iso(text: &str) -> Option<DateTime> {
    DateTime::from_isoformat(text).ok()
}

/// `dt.isoformat()`.
pub fn iso(dt: &DateTime) -> String {
    dt.isoformat(Timespec::Auto)
}

/// A calendar date (`datetime.date`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    pub fn isoformat(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    fn checked(year: i64, month: u32, day: u32) -> Option<Date> {
        ((1..=9999).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month))
        .then_some(Date {
            year: year as i32,
            month,
            day,
        })
    }

    /// `date.fromisoformat(text)` (Python 3.12: `YYYY-MM-DD`, `YYYYMMDD`, `YYYY-Www[-D]`, `YYYYWww[D]`).
    pub fn from_isoformat(text: &str) -> Option<Date> {
        let b = text.as_bytes();
        if !text.is_ascii() || !matches!(b.len(), 7 | 8 | 10) {
            return None;
        }
        let num = |s: &[u8]| -> Option<i64> {
            if s.iter().all(u8::is_ascii_digit) {
                Some(s.iter().fold(0i64, |n, d| n * 10 + i64::from(d - b'0')))
            } else {
                None
            }
        };
        let year = num(&b[..4])?;
        let mut i = 4;
        let dashed = b.get(4) == Some(&b'-');
        if dashed {
            i += 1;
        }
        if b.get(i) == Some(&b'W') {
            let week = num(b.get(i + 1..i + 3)?)?;
            let mut j = i + 3;
            let mut weekday = 1;
            if j < b.len() {
                if dashed {
                    if b[j] != b'-' {
                        return None;
                    }
                    j += 1;
                }
                weekday = num(b.get(j..j + 1)?)?;
                j += 1;
            }
            if j != b.len() || !(1..=7).contains(&weekday) {
                return None;
            }
            // date.fromisocalendar: week 53 only in a year that has it
            if !(1..=9999).contains(&year) || !(1..=53).contains(&week) {
                return None;
            }
            if week == 53 {
                let jan1 = days_from_civil(year, 1, 1).rem_euclid(7); // Monday = 0
                if !(jan1 == 3 || (jan1 == 2 && is_leap(year))) {
                    return None;
                }
            }
            let jan4 = days_from_civil(year, 1, 4);
            let jan4_weekday = jan4.rem_euclid(7);
            let day = jan4 - jan4_weekday + (week - 1) * 7 + (weekday - 1);
            let (y, m, d) = civil_from_days(day);
            return Date::checked(y, m, d);
        }
        let (month, day) = if dashed {
            if b.len() != 10 || b[7] != b'-' {
                return None;
            }
            (num(&b[5..7])?, num(&b[8..10])?)
        } else {
            if b.len() != 8 {
                return None;
            }
            (num(&b[4..6])?, num(&b[6..8])?)
        };
        Date::checked(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?)
    }
}
