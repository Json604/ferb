//! The little of Python's `datetime` FERB's stores need: `datetime.fromisoformat` (Python 3.12,
//! `Modules/_datetimemodule.c`), `isoformat`, and the move to UTC, so stored times read and
//! print exactly as the Python daemon's (decision: ENG-3).

use std::fmt::Write;

/// A `datetime`: naive when `offset` is `None`, else with a fixed offset (seconds east of UTC
/// and, as Python's `timezone` allows, microseconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
    pub offset: Option<(i32, i32)>,
}

/// `isoformat`'s `timespec`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timespec {
    /// Microseconds only when they are not zero (Python's default).
    Auto,
    Seconds,
    Microseconds,
}

/// Python's `ValueError` from `fromisoformat` or the constructor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid;

/// Python's `OverflowError("date value out of range")` from `astimezone`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutOfRange;

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Python's proleptic ordinal: 0001-01-01 is day 1.
fn ymd_to_ord(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - 1;
    let before: i64 = (1..month).map(|m| i64::from(days_in_month(year, m))).sum();
    y * 365 + y / 4 - y / 100 + y / 400 + before + i64::from(day)
}

fn ord_to_ymd(ordinal: i64) -> (i64, u32, u32) {
    let mut n = ordinal - 1;
    let n400 = n.div_euclid(146_097);
    n = n.rem_euclid(146_097);
    let n100 = n / 36_524;
    n %= 36_524;
    let n4 = n / 1461;
    n %= 1461;
    let n1 = n / 365;
    n %= 365;
    let year = n400 * 400 + n100 * 100 + n4 * 4 + n1 + 1;
    if n1 == 4 || n100 == 4 {
        return (year - 1, 12, 31);
    }
    let y = year as i32;
    let mut month = 1;
    while n >= i64::from(days_in_month(y, month)) {
        n -= i64::from(days_in_month(y, month));
        month += 1;
    }
    (year, month, n as u32 + 1)
}

/// The ordinal of the Monday starting ISO week 1 of `year` (`iso_week1_monday`).
fn iso_week1_monday(year: i32) -> i64 {
    let first = ymd_to_ord(year, 1, 1);
    let weekday = (first + 6).rem_euclid(7); // Monday = 0
    let monday = first - weekday;
    if weekday > 3 {
        monday + 7
    } else {
        monday
    }
}

/// The byte at `i`, or NUL past the end: the C code reads a NUL-terminated buffer.
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `parse_digits`: `count` digits from `i`; the value and the index after them.
fn digits(s: &[u8], i: usize, count: usize) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for k in 0..count {
        let c = at(s, i + k);
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(c - b'0');
    }
    Some((value, i + count))
}

/// `_find_isoformat_datetime_separator`.
fn separator(s: &[u8]) -> Option<usize> {
    let len = s.len();
    if len == 7 {
        return Some(7);
    }
    if s[4] == b'-' {
        if s[5] == b'W' {
            if len < 8 {
                return None;
            }
            if len > 8 && s[8] == b'-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && s[10].is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if s[4] == b'W' {
        let mut idx = 7;
        while idx < len && s[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

/// `parse_isoformat_date` over `s[..len]`: (year, month, day), not yet range-checked except
/// for ISO weeks.
fn parse_date(s: &[u8], len: usize) -> Result<(i32, u32, u32), Invalid> {
    let (year, mut p) = digits(s, 0, 4).ok_or(Invalid)?;
    let year = year as i32;
    let dashed = at(s, p) == b'-';
    if dashed {
        p += 1;
    }
    if at(s, p) == b'W' {
        p += 1;
        let (week, after) = digits(s, p, 2).ok_or(Invalid)?;
        p = after;
        let mut weekday = 1;
        if p < len {
            if dashed {
                if at(s, p) != b'-' {
                    return Err(Invalid);
                }
                p += 1;
            }
            weekday = digits(s, p, 1).ok_or(Invalid)?.0;
        }
        // iso_to_ymd
        if !(1..=9999).contains(&year) || week == 0 || week > 53 {
            return Err(Invalid);
        }
        if week == 53 {
            // only a year whose 1 January is a Thursday (or a leap year's a Wednesday) has it
            let first = (ymd_to_ord(year, 1, 1) + 6).rem_euclid(7);
            if !(first == 3 || (first == 2 && is_leap(year))) {
                return Err(Invalid);
            }
        }
        if !(1..=7).contains(&weekday) {
            return Err(Invalid);
        }
        let ordinal = iso_week1_monday(year) + i64::from(week - 1) * 7 + i64::from(weekday) - 1;
        let (y, m, d) = ord_to_ymd(ordinal);
        return Ok((y as i32, m, d));
    }
    let (month, after) = digits(s, p, 2).ok_or(Invalid)?;
    p = after;
    if dashed {
        if at(s, p) != b'-' {
            return Err(Invalid);
        }
        p += 1;
    }
    let (day, _) = digits(s, p, 2).ok_or(Invalid)?;
    Ok((year, month, day))
}

/// `parse_hh_mm_ss_ff` over `s[start..end]`: the values and whether text was left over.
fn parse_hms(s: &[u8], start: usize, end: usize) -> Result<([u32; 4], bool), Invalid> {
    let mut vals = [0u32; 4];
    let mut p = start;
    let mut has_separator = true;
    let mut fraction = false;
    for i in 0..3 {
        let (value, after) = digits(s, p, 2).ok_or(Invalid)?;
        vals[i] = value;
        p = after;
        let c = at(s, p);
        p += 1;
        if i == 0 {
            has_separator = c == b':';
        }
        if p >= end {
            return Ok((vals, c != 0));
        } else if has_separator && c == b':' {
            continue;
        } else if c == b'.' || c == b',' {
            fraction = true;
            break;
        } else if !has_separator {
            p -= 1;
        } else {
            return Err(Invalid);
        }
    }
    let _ = fraction; // the C loop also falls through here after three fields
    if p >= end {
        return Err(Invalid);
    }
    let to_parse = (end - p).min(6);
    let (micro, after) = digits(s, p, to_parse).ok_or(Invalid)?;
    vals[3] = micro;
    p = after;
    if to_parse < 6 {
        vals[3] *= [100_000, 10_000, 1000, 100, 10][to_parse - 1];
    }
    while at(s, p).is_ascii_digit() {
        p += 1;
    }
    Ok((vals, at(s, p) != 0))
}

impl DateTime {
    /// A checked naive or aware datetime (the ranges `datetime(...)` accepts).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        microsecond: u32,
        offset: Option<(i32, i32)>,
    ) -> Result<DateTime, Invalid> {
        let date_ok = (1..=9999).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month);
        if !date_ok || hour > 23 || minute > 59 || second > 59 || microsecond > 999_999 {
            return Err(Invalid);
        }
        if let Some((seconds, micros)) = offset {
            // timezone(): strictly between -24 h and 24 h
            let total = i64::from(seconds) * 1_000_000 + i64::from(micros);
            if total.abs() >= 86_400_000_000 {
                return Err(Invalid);
            }
        }
        Ok(DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            microsecond,
            offset,
        })
    }

    /// `datetime.fromisoformat(text)` as Python 3.12 reads it; `Invalid` is its `ValueError`.
    pub fn from_isoformat(text: &str) -> Result<DateTime, Invalid> {
        let s = text.as_bytes();
        let len = s.len();
        if len < 7 {
            return Err(Invalid);
        }
        // The C code works on UTF-8; the date part must be ASCII for any of its formats.
        if !s[..7].is_ascii() {
            return Err(Invalid);
        }
        let sep = separator(s).ok_or(Invalid)?;
        if sep > len {
            return Err(Invalid);
        }
        let (year, month, day) = parse_date(s, sep)?;
        let mut time = [0u32; 4];
        let mut offset = None;
        if sep < len {
            // one separator character of any kind, counted in UTF-8 bytes
            let width = match s[sep] {
                b if b & 0x80 == 0 => 1,
                b if b & 0xf0 == 0xe0 => 3,
                b if b & 0xf0 == 0xf0 => 4,
                _ => 2,
            };
            let start = sep + width;
            if start > len {
                return Err(Invalid);
            }
            // parse_isoformat_time: the offset starts at the first Z, + or -
            let mut tz = start;
            loop {
                if matches!(at(s, tz), b'Z' | b'+' | b'-') {
                    break;
                }
                tz += 1;
                if tz >= len {
                    break;
                }
            }
            let tz = tz.min(len);
            let (vals, left) = parse_hms(s, start, tz)?;
            time = vals;
            if tz == len {
                if left {
                    return Err(Invalid);
                }
            } else if s[tz] == b'Z' {
                if tz + 1 != len {
                    return Err(Invalid);
                }
                offset = Some((0, 0));
            } else {
                let sign: i32 = if s[tz] == b'-' { -1 } else { 1 };
                let (vals, left) = parse_hms(s, tz + 1, len)?;
                if left {
                    return Err(Invalid);
                }
                let seconds = sign * (vals[0] * 3600 + vals[1] * 60 + vals[2]) as i32;
                // tzinfo_from_isoformat_results: zero whole seconds is UTC, whatever the
                // offset's microseconds say (Python 3.12 drops them there)
                let micros = if seconds == 0 {
                    0
                } else {
                    sign * vals[3] as i32
                };
                offset = Some((seconds, micros));
            }
        }
        DateTime::new(year, month, day, time[0], time[1], time[2], time[3], offset)
    }

    /// Microseconds since 0001-01-01T00:00:00 of the wall time shown.
    fn wall_micros(&self) -> i64 {
        let days = ymd_to_ord(self.year, self.month, self.day) - 1;
        let seconds = days * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second);
        seconds * 1_000_000 + i64::from(self.microsecond)
    }

    fn from_wall_micros(micros: i64, offset: Option<(i32, i32)>) -> Result<DateTime, OutOfRange> {
        let days = micros.div_euclid(86_400_000_000);
        let rest = micros.rem_euclid(86_400_000_000);
        let (year, month, day) = ord_to_ymd(days + 1);
        if !(1..=9999).contains(&year) {
            return Err(OutOfRange);
        }
        let seconds = rest / 1_000_000;
        Ok(DateTime {
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

    /// `self.astimezone(timezone.utc)` for an aware time (a naive one is returned as it is: the
    /// stores never convert one).
    pub fn to_utc(&self) -> Result<DateTime, OutOfRange> {
        let Some((seconds, micros)) = self.offset else {
            return Ok(*self);
        };
        let shift = i64::from(seconds) * 1_000_000 + i64::from(micros);
        DateTime::from_wall_micros(self.wall_micros() - shift, Some((0, 0)))
    }

    /// This time moved by `seconds` and `micros`, the offset kept (a `timedelta` added).
    pub fn plus(&self, seconds: i64, micros: i64) -> Result<DateTime, OutOfRange> {
        DateTime::from_wall_micros(
            self.wall_micros() + seconds * 1_000_000 + micros,
            self.offset,
        )
    }

    /// `self.isoformat(timespec=...)`.
    pub fn isoformat(&self, timespec: Timespec) -> String {
        let mut out = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        let micros = match timespec {
            Timespec::Auto => self.microsecond != 0,
            Timespec::Seconds => false,
            Timespec::Microseconds => true,
        };
        if micros {
            let _ = write!(out, ".{:06}", self.microsecond);
        }
        if let Some((seconds, tz_micros)) = self.offset {
            let total = i64::from(seconds) * 1_000_000 + i64::from(tz_micros);
            let sign = if total < 0 { '-' } else { '+' };
            let total = total.abs();
            let (whole, part) = (total / 1_000_000, total % 1_000_000);
            let _ = write!(out, "{sign}{:02}:{:02}", whole / 3600, whole / 60 % 60);
            if whole % 60 != 0 || part != 0 {
                let _ = write!(out, ":{:02}", whole % 60);
            }
            if part != 0 {
                let _ = write!(out, ".{part:06}");
            }
        }
        out
    }

    /// `f"{self:%Y%m%d-%H%M}"` (`ferb/store/salvage.py:130`).
    pub fn compact_minute(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }

    /// `datetime.now()`: this device's local wall time, naive.
    #[cfg(unix)]
    pub fn now_local() -> DateTime {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = since.as_secs() as libc::time_t;
        let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
        // SAFETY: `secs` is a valid time_t and `tm` is writable memory of the right size, which
        // localtime_r fills when it returns non-null.
        let tm = unsafe {
            if libc::localtime_r(&secs, tm.as_mut_ptr()).is_null() {
                return DateTime::utc_from_unix(since.as_secs() as i64, since.subsec_micros());
            }
            tm.assume_init()
        };
        DateTime {
            year: tm.tm_year + 1900,
            month: tm.tm_mon as u32 + 1,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            second: (tm.tm_sec as u32).min(59),
            microsecond: since.subsec_micros(),
            offset: None,
        }
    }

    /// `datetime.now()` where the C library's `localtime_r` is not there: UTC's wall time, naive.
    #[cfg(not(unix))]
    pub fn now_local() -> DateTime {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        DateTime {
            offset: None,
            ..DateTime::utc_from_unix(since.as_secs() as i64, since.subsec_micros())
        }
    }

    /// `datetime.fromtimestamp(secs, timezone.utc)` with `micros` added.
    pub fn utc_from_unix(secs: i64, micros: u32) -> DateTime {
        let epoch = (ymd_to_ord(1970, 1, 1) - 1) * 86_400_000_000;
        DateTime::from_wall_micros(epoch + secs * 1_000_000 + i64::from(micros), Some((0, 0)))
            .expect("a system time is within years 1..9999")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calendar_round_trips() {
        for ordinal in [1, 59, 60, 366, 730_120, 739_930, 3_652_059] {
            let (y, m, d) = ord_to_ymd(ordinal);
            assert_eq!(ymd_to_ord(y as i32, m, d), ordinal);
        }
        assert_eq!(ord_to_ymd(1), (1, 1, 1));
        assert_eq!(ord_to_ymd(3_652_059), (9999, 12, 31));
    }

    #[test]
    fn a_time_prints_as_python_prints_it() {
        let t = DateTime::from_isoformat("2026-11-09T10:11:12.5+05:30").unwrap();
        assert_eq!(
            t.isoformat(Timespec::Auto),
            "2026-11-09T10:11:12.500000+05:30"
        );
        assert_eq!(
            t.to_utc().unwrap().isoformat(Timespec::Microseconds),
            "2026-11-09T04:41:12.500000+00:00"
        );
        assert_eq!(t.isoformat(Timespec::Seconds), "2026-11-09T10:11:12+05:30");
        assert_eq!(t.compact_minute(), "20261109-1011");
    }
}
