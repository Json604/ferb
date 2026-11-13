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

// ---------------------------------------------------------------------------------------------------------------
// one representation for stored and wire times, and the way to and from it
// ---------------------------------------------------------------------------------------------------------------

/// Why a time can't be the other type's value. Nothing is ever rounded or moved to make it fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotRepresentable {
    /// A naive Python `datetime` (no offset): this module's [`DateTime`] is always aware.
    Naive,
    /// A Python offset with microseconds (`timezone(timedelta(microseconds=1))` is legal): chrono's offsets are whole
    /// seconds.
    SubsecondOffset,
    /// Outside Python's years 1 to 9999, or an offset of a whole day or more.
    OutOfRange,
}

/// The time as `ferb_py::time::DateTime`, the representation for stored and wire times: it holds every value
/// Python's `datetime.fromisoformat` reads and `isoformat` writes (naive or aware, offsets to the microsecond, years 1
/// to 9999), so the text FERB stores or sends reads back to the same value and prints to the same bytes. A chrono
/// [`DateTime`] is the in-memory form for clocks and arithmetic. `Err` only for a year Python can't hold.
pub fn to_py(dt: &DateTime) -> Result<ferb_py::time::DateTime, NotRepresentable> {
    ferb_py::time::DateTime::new(
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        // a leap second (nanosecond >= 1e9) has no Python form
        (dt.nanosecond() < 1_000_000_000)
            .then_some(dt.nanosecond() / 1000)
            .ok_or(NotRepresentable::OutOfRange)?,
        Some((dt.offset().local_minus_utc(), 0)),
    )
    .map_err(|_| NotRepresentable::OutOfRange)
}

/// A stored or wire time as a chrono [`DateTime`], losslessly: refused (never guessed) when it is naive or its offset
/// has microseconds.
pub fn from_py(t: &ferb_py::time::DateTime) -> Result<DateTime, NotRepresentable> {
    let (seconds, micros) = t.offset.ok_or(NotRepresentable::Naive)?;
    if micros != 0 {
        return Err(NotRepresentable::SubsecondOffset);
    }
    let offset = FixedOffset::east_opt(seconds).ok_or(NotRepresentable::OutOfRange)?;
    let naive = NaiveDate::from_ymd_opt(t.year, t.month, t.day)
        .and_then(|d| d.and_hms_micro_opt(t.hour, t.minute, t.second, t.microsecond))
        .ok_or(NotRepresentable::OutOfRange)?;
    offset
        .from_local_datetime(&naive)
        .single()
        .ok_or(NotRepresentable::OutOfRange)
}

#[cfg(test)]
mod conversions {
    use super::*;
    use ferb_py::time::{DateTime as PyTime, Timespec};

    fn both_ways(text: &str) {
        let py = PyTime::from_isoformat(text).unwrap();
        let ours = from_py(&py).unwrap();
        assert_eq!(isoformat(&ours), py.isoformat(Timespec::Auto), "{text}");
        assert_eq!(to_py(&ours).unwrap(), py, "{text}: back to the same value");
        assert_eq!(
            parse(text),
            Some(ours),
            "{text}: this module reads the same time"
        );
    }

    #[test]
    fn every_time_both_can_hold_goes_both_ways_unchanged() {
        // fixtures/core/types.json's datetimes (Python wrote them) and the edges of what both hold
        for text in [
            "2026-03-04T09:30:15+00:00",
            "2026-03-04T15:00:15.000250+05:30",
            "2025-12-31T23:59:59.999999-08:00",
            "2026-01-01T00:00:00+00:00",
            "1999-07-08T01:02:03.040000+05:30",
            "2026-06-01T12:00:00.000007-03:30",
            "0001-01-01T00:00:00+00:00",
            "9999-12-31T23:59:59.999999+00:00",
            "2024-02-29T12:00:00+23:59:59",
            "2024-02-29T12:00:00-23:59:59",
            "2026-07-01T00:00:00+05:45:30",
        ] {
            both_ways(text);
        }
    }

    #[test]
    fn a_time_one_side_cant_hold_is_refused_not_rounded() {
        let naive = PyTime::from_isoformat("2026-03-04T09:30:15").unwrap();
        assert_eq!(from_py(&naive), Err(NotRepresentable::Naive));
        let sub = PyTime::new(2026, 3, 4, 9, 30, 15, 0, Some((3600, 1))).unwrap();
        assert_eq!(from_py(&sub), Err(NotRepresentable::SubsecondOffset));
        let far = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(10000, 1, 1, 0, 0, 0)
            .unwrap();
        assert_eq!(to_py(&far), Err(NotRepresentable::OutOfRange));
        let before = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(0, 12, 31, 23, 59, 59)
            .unwrap();
        assert_eq!(to_py(&before), Err(NotRepresentable::OutOfRange));
    }

    #[test]
    fn the_core_fixtures_datetimes_convert_without_a_difference() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/core/types.json"
        ))
        .unwrap();
        let fx: serde_json::Value = serde_json::from_str(&text).unwrap();
        let cases = fx["datetimes"].as_array().unwrap();
        assert_eq!(cases.len(), 6);
        for case in cases {
            let iso = case["iso"].as_str().unwrap();
            both_ways(iso);
            let ours = from_py(&PyTime::from_isoformat(iso).unwrap()).unwrap();
            assert_eq!(py_str(&ours), case["str"].as_str().unwrap());
        }
    }

    #[test]
    fn the_stores_fromisoformat_cases_convert_or_are_refused_by_name() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/store/tables.json"
        ))
        .unwrap();
        let fx: serde_json::Value = serde_json::from_str(&text).unwrap();
        let (mut both, mut naive, mut refused) = (0, 0, 0);
        for case in fx["python_says"]["times"].as_array().unwrap() {
            let (text, said) = (case[0].as_str().unwrap(), &case[1]);
            let Ok(py) = PyTime::from_isoformat(text) else {
                assert!(said.is_null(), "{text}");
                continue;
            };
            match from_py(&py) {
                Ok(ours) => {
                    both += 1;
                    assert_eq!(isoformat(&ours), said["iso"].as_str().unwrap(), "{text}");
                    assert_eq!(to_py(&ours).unwrap(), py, "{text}");
                }
                Err(NotRepresentable::Naive) => {
                    naive += 1;
                    assert_eq!(said["aware"], false, "{text}");
                }
                Err(e) => {
                    refused += 1;
                    assert_eq!(said["aware"], true, "{text}: {e:?}");
                }
            }
        }
        assert!(both > 0 && naive > 0, "{both} {naive} {refused}");
    }
}
