//! The engine's times as FERB.app reads them (`LiveProgress.time`: ISO 8601 with an offset, fractional seconds
//! optional), as seconds since 2001-01-01T00:00:00Z (Foundation's reference date, a `Date`'s own number), so the fold
//! compares and orders exactly as the Mac's does (`EntryFold.date`).

/// Seconds between 1970-01-01 and 2001-01-01 (UTC).
pub const REFERENCE_UNIX: f64 = 978_307_200.0;

/// `Date.distantPast` as seconds since the reference date.
pub const DISTANT_PAST: f64 = -63_114_076_800.0;

/// What an unreadable time reads as on the Mac (`Date(timeIntervalSince1970: 0)`).
pub const UNREADABLE: f64 = -REFERENCE_UNIX;

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(s: &[u8]) -> Option<i64> {
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(s).ok()?.parse().ok()
}

/// An ISO 8601 time with an offset (`2026-01-01T09:00:00+00:00`, `…T09:00:00.123Z`), as seconds since the reference
/// date; None when it isn't one.
pub fn parse(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (digits(&b[0..4])?, digits(&b[5..7])?, digits(&b[8..10])?);
    let (h, mi, s) = (
        digits(&b[11..13])?,
        digits(&b[14..16])?,
        digits(&b[17..19])?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut i = 19;
    let mut frac = 0.0f64;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return None;
        }
        frac = format!("0.{}", std::str::from_utf8(&b[start..end]).ok()?)
            .parse()
            .ok()?;
        i = end;
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') => {
            let rest = &b[i + 1..];
            let (oh, om) = match rest.len() {
                5 if rest[2] == b':' => (digits(&rest[0..2])?, digits(&rest[3..5])?),
                4 => (digits(&rest[0..2])?, digits(&rest[2..4])?),
                2 => (digits(rest)?, 0),
                _ => return None,
            };
            let o = oh * 3600 + om * 60;
            if *sign == b'-' {
                -o
            } else {
                o
            }
        }
        _ => return None,
    };
    let unix = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset;
    Some(unix as f64 - REFERENCE_UNIX + frac)
}

/// `EntryFold.date`: the time, or what an unreadable one reads as.
pub fn date(text: &str) -> f64 {
    parse(text).unwrap_or(UNREADABLE)
}

/// The calendar day (days since 1970-01-01) a time falls on, at a fixed offset from UTC in seconds: the phone's own
/// zone as the app gives it (`Calendar.current` on the Mac).
pub fn day(at: f64, utc_offset_s: i32) -> i64 {
    ((at + REFERENCE_UNIX + f64::from(utc_offset_s)) / 86_400.0).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_times_as_foundation_does() {
        // values printed by Foundation's ISO8601FormatStyle(includingFractionalSeconds: true) on 2026-11-16
        assert_eq!(parse("2026-01-01T09:00:00+00:00"), Some(788_950_800.0));
        assert_eq!(parse("2026-01-01T09:00:00Z"), Some(788_950_800.0));
        assert_eq!(parse("2026-01-01T09:00:00.123Z"), Some(788_950_800.123));
        assert_eq!(
            parse("2026-01-01T09:00:00.123456+05:30"),
            Some(788_931_000.123_456)
        );
        assert_eq!(parse("not a time"), None);
        assert_eq!(date(""), UNREADABLE);
    }

    #[test]
    fn a_day_is_the_zones_day() {
        let at = parse("2026-01-01T20:00:00Z").unwrap();
        assert_eq!(day(at, 0), day(parse("2026-01-01T00:00:00Z").unwrap(), 0));
        assert_eq!(day(at, 19_800), day(at, 0) + 1); // 01:30 the next day in +05:30
    }
}
