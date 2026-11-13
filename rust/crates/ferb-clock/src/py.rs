//! The few pieces of Python's own behavior this part leans on and `ferb-py` doesn't hold yet: `timedelta` from a
//! float (its exact rounding), `strftime` in the C locale, `isoformat(timespec=...)`, `repr(str)`, `str.casefold()`,
//! `str.splitlines()`, `re`'s `\d` and `\s` for the hand-written matchers, `int()` of Unicode digits and `format(x,
//! "g")`. Unicode answers come from Python 3.12's tables (`generated.rs`, made by `rust/fixtures/clock/make.py`).

use std::fmt::Write;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, TimeDelta, Timelike};

use crate::generated::{CASEFOLD, UNPRINTABLE, WORD};

/// An aware time with its own UTC offset, as Python's `datetime` with a fixed `timezone` (what FERB's clocks,
/// stores and `astimezone()` give).
pub type Time = DateTime<FixedOffset>;

pub const US_PER_SECOND: i64 = 1_000_000;
pub const US_PER_MINUTE: i64 = 60_000_000;
const MAX_DAYS: i128 = 999_999_999;

/// Python's `OverflowError` from `timedelta(...)` or from date arithmetic, with its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overflow(pub String);

/// `timedelta(<unit>=value)` in microseconds for a float `value`, as CPython's `delta_new` / `accum` compute it
/// (`Modules/_datetimemodule.c`): the whole part exactly, the fraction through doubles, the leftover rounded half to
/// even against the running total. `factor`: microseconds per unit.
pub fn timedelta_us(value: f64, factor: i64) -> Result<i64, Overflow> {
    if !value.is_finite() {
        return Err(Overflow(if value.is_nan() {
            "cannot convert float NaN to integer".into()
        } else {
            "cannot convert float infinity to integer".into()
        }));
    }
    // Python's int is unbounded; a total past i128 has far more days than a C int holds, which is the error
    // Python raises for it (`microseconds_to_delta`)
    let too_large = || Overflow("Python int too large to convert to C int".into());
    let int_part = value.trunc();
    let frac = value - int_part;
    if int_part.abs() >= i128::MAX as f64 {
        return Err(too_large());
    }
    let mut x: i128 = (int_part as i128)
        .checked_mul(i128::from(factor))
        .ok_or_else(too_large)?;
    let mut leftover = 0.0f64;
    if frac != 0.0 {
        let d = factor as f64 * frac;
        let ip = d.trunc();
        x = x.checked_add(ip as i128).ok_or_else(too_large)?;
        leftover += d - ip;
    }
    if leftover != 0.0 {
        let mut whole = leftover.round(); // C round(): half away from zero
        if (whole - leftover).abs() == 0.5 {
            let odd = (x & 1) as f64;
            whole = 2.0 * ((leftover + odd) * 0.5).round() - odd;
        }
        x = x.checked_add(whole as i128).ok_or_else(too_large)?;
    }
    check_days(x.div_euclid(86_400_000_000))?;
    // A timedelta past i64 microseconds (over ~106.75 million days) is legal in Python, but adding it to any datetime
    // raises this OverflowError (year 9999 is ~3.65 million days): said here, never wrapped into a wrong value.
    i64::try_from(x).map_err(|_| Overflow("date value out of range".into()))
}

/// `microseconds_to_delta`'s checks: the day count must fit a C int, then +-999999999.
fn check_days(days: i128) -> Result<(), Overflow> {
    if days > i128::from(i32::MAX) || days < i128::from(i32::MIN) {
        return Err(Overflow("Python int too large to convert to C int".into()));
    }
    if days.abs() > MAX_DAYS {
        return Err(Overflow(format!(
            "days={days}; must have magnitude <= 999999999"
        )));
    }
    Ok(())
}

/// `timedelta(days=n)` in microseconds (an int argument).
pub fn days_us(days: i64) -> Result<i64, Overflow> {
    check_days(i128::from(days))?;
    Ok(days * 86_400 * US_PER_SECOND)
}

const MIN_YEAR: i32 = 1;
const MAX_YEAR: i32 = 9999;

fn in_range(year: i32) -> bool {
    (MIN_YEAR..=MAX_YEAR).contains(&year)
}

/// `t + timedelta(microseconds=us)`: the same offset, "date value out of range" past year 9999 or before year 1.
pub fn plus_us(t: &Time, us: i64) -> Result<Time, Overflow> {
    let out = t
        .checked_add_signed(TimeDelta::microseconds(us))
        .ok_or_else(|| Overflow("date value out of range".into()))?;
    if in_range(out.naive_local().year()) {
        Ok(out)
    } else {
        Err(Overflow("date value out of range".into()))
    }
}

/// `date + timedelta(days=n)`.
pub fn date_plus(d: NaiveDate, days: i64) -> Result<NaiveDate, Overflow> {
    days_us(days)?;
    d.checked_add_signed(TimeDelta::days(days))
        .filter(|x| in_range(x.year()))
        .ok_or_else(|| Overflow("date value out of range".into()))
}

/// `(a - b).total_seconds()`: microseconds divided as Python divides two ints (correctly rounded).
pub fn total_seconds(a: &Time, b: &Time) -> f64 {
    us_between(a, b) as f64 / 1e6
}

/// `a - b` in microseconds.
pub fn us_between(a: &Time, b: &Time) -> i64 {
    (a.timestamp_micros()).wrapping_sub(b.timestamp_micros())
}

/// Python's `round(x)` of a float: half to even, as an int.
pub fn round_even(x: f64) -> i64 {
    x.round_ties_even() as i64
}

// ---------- formatting ----------

const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `%z` of a fixed offset: `+0530`, `-0330`, `+000000`-style seconds when they aren't zero.
pub fn offset_z(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let a = seconds.abs();
    let mut out = format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60);
    if a % 60 != 0 {
        let _ = write!(out, "{:02}", a % 60);
    }
    out
}

/// What `tzname()` (`%Z`) of Python's `timezone(offset, name)` says: the name, else `UTC` for a zero offset, else
/// `UTC+05:30`.
pub fn fixed_tzname(seconds: i32, name: Option<&str>) -> String {
    if let Some(name) = name {
        return name.to_string();
    }
    if seconds == 0 {
        return "UTC".into();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let a = seconds.abs();
    let mut out = format!("UTC{sign}{:02}:{:02}", a / 3600, a / 60 % 60);
    if a % 60 != 0 {
        let _ = write!(out, ":{:02}", a % 60);
    }
    out
}

/// `strftime` in the C locale for the directives FERB uses (`%a %A %b %d %H %M %S %Y %m %Z %z %%`), of a local wall
/// time with its offset and zone name.
pub fn strftime(local: &NaiveDateTime, offset: i32, tzname: &str, pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('a') => out.push_str(&DAYS[local.weekday().num_days_from_monday() as usize][..3]),
            Some('A') => out.push_str(DAYS[local.weekday().num_days_from_monday() as usize]),
            Some('b') => out.push_str(MONTHS[local.month0() as usize]),
            Some('d') => {
                let _ = write!(out, "{:02}", local.day());
            }
            Some('m') => {
                let _ = write!(out, "{:02}", local.month());
            }
            Some('Y') => {
                let _ = write!(out, "{}", local.year());
            }
            Some('H') => {
                let _ = write!(out, "{:02}", local.hour());
            }
            Some('M') => {
                let _ = write!(out, "{:02}", local.minute());
            }
            Some('S') => {
                let _ = write!(out, "{:02}", local.second());
            }
            Some('Z') => out.push_str(tzname),
            Some('z') => out.push_str(&offset_z(offset)),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// `strftime` of an aware time in its own offset (Python's `f"{t:...}"`); `%Z` as an unnamed fixed offset says it.
pub fn fmt(t: &Time, pattern: &str) -> String {
    let offset = t.offset().local_minus_utc();
    strftime(
        &t.naive_local(),
        offset,
        &fixed_tzname(offset, None),
        pattern,
    )
}

/// `isoformat(timespec=...)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    Auto,
    Minutes,
    Seconds,
}

/// `t.isoformat()` / `t.isoformat(timespec="minutes" | "seconds")`.
pub fn isoformat(t: &Time, spec: Spec) -> String {
    let l = t.naive_local();
    let mut out = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        l.year(),
        l.month(),
        l.day(),
        l.hour(),
        l.minute()
    );
    if spec != Spec::Minutes {
        let _ = write!(out, ":{:02}", l.second());
        let micros = l.nanosecond() / 1000;
        if spec == Spec::Auto && micros != 0 {
            let _ = write!(out, ".{micros:06}");
        }
    }
    let offset = t.offset().local_minus_utc();
    let sign = if offset < 0 { '-' } else { '+' };
    let a = offset.abs();
    let _ = write!(out, "{sign}{:02}:{:02}", a / 3600, a / 60 % 60);
    if a % 60 != 0 {
        let _ = write!(out, ":{:02}", a % 60);
    }
    out
}

/// `t.isoformat()`.
pub fn iso(t: &Time) -> String {
    isoformat(t, Spec::Auto)
}

/// `datetime.fromisoformat(text)` for an aware time (what FERB stores); naive text is refused here.
pub fn parse(text: &str) -> Option<Time> {
    let d = ferb_py::time::DateTime::from_isoformat(text).ok()?;
    let (seconds, micros) = d.offset?;
    if micros != 0 {
        return None;
    }
    let offset = FixedOffset::east_opt(seconds)?;
    let naive = NaiveDate::from_ymd_opt(d.year, d.month, d.day)?.and_hms_micro_opt(
        d.hour,
        d.minute,
        d.second,
        d.microsecond,
    )?;
    naive.and_local_timezone(offset).single()
}

/// `datetime.fromisoformat(text)` that may be naive: `Err` for text Python refuses, `Ok(None)` for a naive time.
pub fn parse_maybe_naive(text: &str) -> Result<Option<Time>, ferb_py::time::Invalid> {
    let d = ferb_py::time::DateTime::from_isoformat(text)?;
    if d.offset.is_none() {
        return Ok(None);
    }
    parse(text).map(Some).ok_or(ferb_py::time::Invalid)
}

/// The UTC time of an aware time, as `astimezone(timezone.utc)`.
pub fn utc(t: &Time) -> Time {
    t.with_timezone(&FixedOffset::east_opt(0).expect("zero offset"))
}

// ---------- strings ----------

/// `repr(text)` for a `str`, with Python 3.12's printable table.
pub fn repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !is_printable(c) => {
                let n = c as u32;
                if n < 0x100 {
                    let _ = write!(out, "\\x{n:02x}");
                } else if n < 0x10000 {
                    let _ = write!(out, "\\u{n:04x}");
                } else {
                    let _ = write!(out, "\\U{n:08x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let n = c as u32;
    ranges
        .binary_search_by(|&(a, b)| {
            if n < a {
                std::cmp::Ordering::Greater
            } else if n > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `re`'s `\w` for a `str` pattern.
pub fn is_word(c: char) -> bool {
    in_ranges(WORD, c)
}

/// `c.isprintable()` (a space is printable).
pub fn is_printable(c: char) -> bool {
    !in_ranges(UNPRINTABLE, c)
}

/// `text.casefold()`.
pub fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match CASEFOLD.binary_search_by_key(&(c as u32), |&(k, _)| k) {
            Ok(i) => out.push_str(CASEFOLD[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}

/// `text.splitlines()`: the line ends Python's `str` knows.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let end = match c {
            '\r' => {
                if let Some(&(_, '\n')) = it.peek() {
                    it.next();
                    Some((i, i + 2))
                } else {
                    Some((i, i + 1))
                }
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => Some((i, i + c.len_utf8())),
            _ => None,
        };
        if let Some((cut, next)) = end {
            out.push(&text[start..cut]);
            start = next;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// What `re` and `str` call whitespace.
pub fn is_space(c: char) -> bool {
    ferb_py::value::is_py_space(c)
}

/// `re`'s `\d` (Unicode decimal digits).
pub fn is_digit(c: char) -> bool {
    ferb_py::unicode::is_decimal(c)
}

/// The value of a Unicode decimal digit (Unicode keeps each set of ten contiguous, starting at zero).
pub fn digit_value(c: char) -> u32 {
    let mut first = c as u32;
    while first > 0 {
        match char::from_u32(first - 1) {
            Some(p) if is_digit(p) => first -= 1,
            _ => break,
        }
    }
    (c as u32 - first) % 10
}

/// `int(digits)` for a run of `\d` characters (no sign, no underscores: what the patterns capture).
pub fn int_of(digits: &str) -> i64 {
    digits.chars().fold(0i64, |n, c| {
        n.saturating_mul(10)
            .saturating_add(i64::from(digit_value(c)))
    })
}

/// `text.strip()` with Python's whitespace.
pub fn strip(text: &str) -> &str {
    ferb_py::value::strip(text)
}

/// `text.rstrip()`.
pub fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

/// `" ".join(text.split())`.
pub fn squeeze(text: &str) -> String {
    ferb_py::value::squeeze(text)
}

/// `len(text)`.
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// `text[:n]` by characters.
pub fn prefix(text: &str, n: usize) -> &str {
    ferb_py::value::prefix(text, n)
}

/// `format(x, "g")` for a float (precision 6).
pub fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    if !x.is_finite() {
        return if x.is_nan() {
            "nan".into()
        } else if x > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let sci = format!("{x:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        let m = mantissa.trim_end_matches('0').trim_end_matches('.');
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{m}e{sign}{:02}", exp.abs());
    }
    let digits = (5 - exp).max(0) as usize;
    let fixed = format!("{x:.digits$}");
    if fixed.contains('.') {
        fixed
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        fixed
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_huge_timedelta_is_pythons_overflow_error_not_a_panic() {
        let words = "Python int too large to convert to C int";
        for v in [1e300, -1e300, f64::MAX, f64::MIN, 1e38 * 3.0, 9.3e30] {
            for factor in [1, US_PER_SECOND, 86_400_000_000] {
                assert_eq!(
                    timedelta_us(v, factor),
                    Err(Overflow(words.into())),
                    "timedelta({v} * {factor} us)"
                );
            }
        }
        // the largest whole-day total i64 holds still reads; past it, the date error Python raises where it is used;
        // past 999999999 days, Python's own days error
        assert_eq!(
            timedelta_us(106_751_991.0, 86_400_000_000),
            Ok(106_751_991 * 86_400_000_000)
        );
        assert_eq!(
            timedelta_us(999_999_999.0, 86_400_000_000),
            Err(Overflow("date value out of range".into()))
        );
        assert!(timedelta_us(1_000_000_000.0, 86_400_000_000)
            .unwrap_err()
            .0
            .starts_with("days=1000000000"));
    }

    use super::*;

    #[test]
    fn timedelta_rounds_as_python() {
        assert_eq!(timedelta_us(12.5, US_PER_MINUTE), Ok(750_000_000));
        assert_eq!(timedelta_us(0.0001, US_PER_MINUTE), Ok(6000));
        assert_eq!(timedelta_us(2.5e-8, US_PER_MINUTE), Ok(2)); // 1.5 us: half to even
        assert!(timedelta_us(1e15, US_PER_MINUTE).is_err());
    }

    #[test]
    fn g_formats_as_python() {
        for (x, s) in [
            (2.0, "2"),
            (30.0, "30"),
            (1e7, "1e+07"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (0.5, "0.5"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
        ] {
            assert_eq!(fmt_g(x), s, "{x}");
        }
    }

    #[test]
    fn lines_split_as_python() {
        assert_eq!(
            splitlines("a\r\nb\rc\n\nd\u{2028}e"),
            ["a", "b", "c", "", "d", "e"]
        );
        assert_eq!(splitlines("a\n"), ["a"]);
        assert!(splitlines("").is_empty());
    }
}
