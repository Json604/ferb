//! Timezone-aware datetimes written the way Python writes them.
//!
//! FERB's Python stores and sends `datetime.isoformat()` (ferb/store/spans.py:67,524, ferb/daemon/areas/rpc.py:596)
//! and, for a memory item read by a worker, `str(datetime)` (ferb/orchestrator/evidence.py:360). Both keep the
//! datetime's own UTC offset, write microseconds only when they are not zero, and differ only in the separator.

use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone, Timelike};

/// An aware datetime that keeps its UTC offset, as Python's does.
pub type DateTime = chrono::DateTime<FixedOffset>;

fn write(dt: &DateTime, sep: char) -> String {
    let mut out = format!(
        "{:04}-{:02}-{:02}{sep}{:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    let micros = dt.nanosecond() / 1000;
    if micros != 0 {
        out.push_str(&format!(".{micros:06}"));
    }
    let offset = dt.offset().local_minus_utc();
    let (sign, abs) = if offset < 0 {
        ('-', -offset)
    } else {
        ('+', offset)
    };
    out.push_str(&format!("{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60));
    if abs % 60 != 0 {
        out.push_str(&format!(":{:02}", abs % 60));
    }
    out
}

/// `datetime.isoformat()`: `2026-03-04T09:30:15+00:00`, `2026-03-04T15:00:15.000250+05:30`.
pub fn isoformat(dt: &DateTime) -> String {
    write(dt, 'T')
}

/// `str(datetime)`: the same with a space between date and time.
pub fn py_str(dt: &DateTime) -> String {
    write(dt, ' ')
}

fn digits(s: &str, at: usize, n: usize) -> Option<u32> {
    let part = s.get(at..at + n)?;
    part.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| part.parse().ok())?
}

/// Reads what [`isoformat`] and [`py_str`] write (and a trailing `Z`). An aware datetime only: text without an
/// offset is refused. Python's `fromisoformat` accepts more spellings (week dates, a bare date, basic format).
pub fn parse(s: &str) -> Option<DateTime> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let date =
        NaiveDate::from_ymd_opt(digits(s, 0, 4)? as i32, digits(s, 5, 2)?, digits(s, 8, 2)?)?;
    let mut at = 19;
    let mut micros = 0;
    if b[at] == b'.' {
        let n = b[at + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if n == 0 || n > 6 {
            return None;
        }
        micros = digits(s, at + 1, n)? * 10u32.pow(6 - n as u32);
        at += 1 + n;
    }
    let naive = date.and_hms_micro_opt(
        digits(s, 11, 2)?,
        digits(s, 14, 2)?,
        digits(s, 17, 2)?,
        micros,
    )?;
    let rest = s.get(at..)?;
    let offset = if rest == "Z" {
        0
    } else {
        let sign = match rest.as_bytes().first()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let seconds = match rest.len() {
            6 if rest.as_bytes()[3] == b':' => 0,
            9 if rest.as_bytes()[3] == b':' && rest.as_bytes()[6] == b':' => digits(rest, 7, 2)?,
            _ => return None,
        };
        sign * (digits(rest, 1, 2)? * 3600 + digits(rest, 4, 2)? * 60 + seconds) as i32
    };
    FixedOffset::east_opt(offset)?
        .from_local_datetime(&naive)
        .single()
}

macro_rules! serde_forms {
    ($name:ident, $opt:ident, $write:path) => {
        pub mod $name {
            use super::DateTime;
            use serde::{de::Error, Deserialize, Deserializer, Serializer};

            pub fn serialize<S: Serializer>(dt: &DateTime, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&$write(dt))
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime, D::Error> {
                let text = String::deserialize(d)?;
                super::parse(&text)
                    .ok_or_else(|| D::Error::custom(format!("not an aware datetime: {text:?}")))
            }
        }

        pub mod $opt {
            use super::DateTime;
            use serde::{de::Error, Deserialize, Deserializer, Serializer};

            pub fn serialize<S: Serializer>(
                dt: &Option<DateTime>,
                s: S,
            ) -> Result<S::Ok, S::Error> {
                match dt {
                    Some(dt) => s.serialize_str(&$write(dt)),
                    None => s.serialize_none(),
                }
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(
                d: D,
            ) -> Result<Option<DateTime>, D::Error> {
                match Option::<String>::deserialize(d)? {
                    None => Ok(None),
                    Some(text) => super::parse(&text).map(Some).ok_or_else(|| {
                        D::Error::custom(format!("not an aware datetime: {text:?}"))
                    }),
                }
            }
        }
    };
}

serde_forms!(iso, iso_opt, super::isoformat);
serde_forms!(spaced, spaced_opt, super::py_str);
