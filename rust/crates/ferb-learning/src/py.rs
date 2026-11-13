//! The bits of Python's behavior this part's text depends on that `ferb-py` and `ferb-memory` don't already have:
//! compact sorted JSON (`json.dumps(sort_keys=True, separators=(',', ':'), ensure_ascii=False)`), `round()` to an
//! int, a dict's repr, English `strftime` names, `PurePosixPath.parent`, `str()` of JSON values and the local date.

use ferb_memory::times::{self, LocalZone};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::DateTime;

/// `learning.encoded`: sorted keys, `,` and `:`, text as written (only `"`, `\` and controls escaped).
pub fn encoded(value: &Value) -> String {
    let mut out = String::new();
    write_compact(value, &mut out);
    out
}

fn write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&ferb_py::json::number(n)),
        Value::String(s) => write_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (n, v) in items.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                write_compact(v, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (n, k) in keys.into_iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write_compact(&map[k], out);
            }
            out.push('}');
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `hashlib.sha256(text.encode()).hexdigest()`.
pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `round(x)` to an int: half to even.
pub fn round_int(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        let t = x.trunc();
        if (t as i64) % 2 == 0 {
            t as i64
        } else {
            r as i64
        }
    } else {
        r as i64
    }
}

/// `str(value)` of a JSON value as Python prints it.
pub fn py_str(value: &Value) -> String {
    ferb_py::value::py_str(value)
}

/// `str(d.get(k))`: "None" when missing.
pub fn py_str_opt(value: Option<&Value>) -> String {
    value.map_or_else(|| "None".to_owned(), py_str)
}

/// `repr(dict)` of text keys and int values, in their order: `{'a': 1, 'b': 2}`.
pub fn dict_repr(pairs: &[(String, i64)]) -> String {
    let inner: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{}: {v}", ferb_memory::pystr::repr(k)))
        .collect();
    format!("{{{}}}", inner.join(", "))
}

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

/// Monday = 0.
pub fn weekday(dt: &DateTime) -> usize {
    let days = times::wall_micros(dt).div_euclid(86_400_000_000);
    // 1970-01-01 was a Thursday (3)
    (days + 3).rem_euclid(7) as usize
}

/// `dt.astimezone()`: to this device's zone.
pub fn local(dt: &DateTime, zone: &dyn LocalZone) -> DateTime {
    times::astimezone(dt, zone).unwrap_or(*dt)
}

/// `dt.astimezone().date().isoformat()`.
pub fn local_day(dt: &DateTime, zone: &dyn LocalZone) -> String {
    times::ymd(&local(dt, zone))
}

/// `f"{dt:%Y-%m-%d (%A)}"`.
pub fn ymd_weekday(dt: &DateTime) -> String {
    format!("{} ({})", times::ymd(dt), DAYS[weekday(dt)])
}

/// `f"{dt:%d %b %Y}"`.
pub fn day_month_year(dt: &DateTime) -> String {
    format!(
        "{:02} {} {:04}",
        dt.day,
        MONTHS[(dt.month - 1) as usize],
        dt.year
    )
}

/// `f"{dt:%a %d %b}"`.
pub fn short_day(dt: &DateTime) -> String {
    format!(
        "{} {:02} {}",
        &DAYS[weekday(dt)][..3],
        dt.day,
        MONTHS[(dt.month - 1) as usize]
    )
}

/// `dt.isoformat()`.
pub fn iso(dt: &DateTime) -> String {
    times::iso(dt)
}

/// `datetime.fromisoformat(text)`, a `ValueError` as Python raises it.
pub fn parse(text: &str) -> crate::Result<DateTime> {
    times::parse_iso(text).ok_or_else(|| {
        crate::Error::Value(format!(
            "Invalid isoformat string: {}",
            ferb_memory::pystr::repr(text)
        ))
    })
}

/// `a > b` for aware times (a mixed pair raises TypeError, as Python does).
pub fn after(a: &DateTime, b: &DateTime) -> crate::Result<bool> {
    Ok(times::compare(a, b)? == std::cmp::Ordering::Greater)
}

/// `dt - timedelta(microseconds=micros)` / `+`.
pub fn shift(dt: &DateTime, micros: i64) -> DateTime {
    times::plus(dt, micros).unwrap_or(*dt)
}

/// `PurePosixPath(text).parent` as text.
pub fn path_parent(text: &str) -> String {
    let rooted = text.starts_with('/');
    // pathlib keeps exactly two leading slashes, collapses three or more to one
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if rooted {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = text
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() {
        return if root.is_empty() {
            ".".into()
        } else {
            root.into()
        };
    }
    let head = &parts[..parts.len() - 1];
    if head.is_empty() {
        return if root.is_empty() {
            ".".into()
        } else {
            root.into()
        };
    }
    format!("{root}{}", head.join("/"))
}

/// `PurePosixPath(text).name`.
pub fn path_name(text: &str) -> String {
    text.split('/')
        .rfind(|p| !p.is_empty() && *p != ".")
        .unwrap_or("")
        .to_owned()
}

/// Python's simple case mapping to lower (what `re.IGNORECASE` compares by).
pub fn simple_lower(c: char) -> char {
    if c == '\u{130}' {
        return 'i';
    }
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// `re`'s extra case equivalences (`_sre`'s `_ignorecase_fixes`): characters whose lowercase forms match.
fn fix_class(c: char) -> u32 {
    const GROUPS: &[&[u32]] = &[
        &[0x69, 0x131],
        &[0x73, 0x17f],
        &[0xb5, 0x3bc],
        &[0x345, 0x3b9, 0x1fbe],
        &[0x390, 0x1fd3],
        &[0x3b0, 0x1fe3],
        &[0x3b2, 0x3d0],
        &[0x3b5, 0x3f5],
        &[0x3b8, 0x3d1],
        &[0x3ba, 0x3f0],
        &[0x3c0, 0x3d6],
        &[0x3c1, 0x3f1],
        &[0x3c2, 0x3c3],
        &[0x3c6, 0x3d5],
        &[0x1e61, 0x1e9b],
        &[0xfb05, 0xfb06],
    ];
    let l = simple_lower(c) as u32;
    for g in GROUPS {
        if g.contains(&l) {
            return g[0];
        }
    }
    l
}

/// Whether two characters match under `re.IGNORECASE` (Unicode).
pub fn same_ignoring_case(a: char, b: char) -> bool {
    a == b || fix_class(a) == fix_class(b)
}
