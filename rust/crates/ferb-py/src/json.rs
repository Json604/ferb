//! JSON as Python's `json` module writes and judges it, for the frames, columns and files FERB
//! shares with the Python build (decision: ENG-3).
//!
//! One writer (`dumps`, `dumps_utf8`, `dumps_with`) for anything `serde` can serialize: Python's
//! `", "` and `": "` separators, `ensure_ascii`, `sort_keys`, `indent`, and floats as
//! `repr(float)`.
//!
//! Reading uses `serde_json` (key order kept). Known differences from `json.loads`, none of
//! which FERB writes: `NaN`/`Infinity`, integers beyond 64 bits (read as floats) and lone
//! `\uD800`-style surrogates are refused or lose precision here.

use std::io;

use serde::ser::{Serialize, Serializer};
use serde_json::ser::{CharEscape, Formatter};
use serde_json::Value;

/// How `json.dumps` is called at one site.
#[derive(Clone, Copy, Debug, Default)]
pub struct Style {
    /// `ensure_ascii` (Python's default is true).
    pub ascii: bool,
    pub sort_keys: bool,
    pub indent: Option<usize>,
}

/// `json.dumps(value)`: `", "` and `": "`, everything outside `' '..='~'` as `\uXXXX`.
pub fn dumps<T: Serialize + ?Sized>(value: &T) -> String {
    dumps_with(
        value,
        Style {
            ascii: true,
            ..Style::default()
        },
    )
}

/// `json.dumps(value, ensure_ascii=False)`: text as written (UTF-8).
pub fn dumps_utf8<T: Serialize + ?Sized>(value: &T) -> String {
    dumps_with(value, Style::default())
}

pub fn dumps_with<T: Serialize + ?Sized>(value: &T, style: Style) -> String {
    let mut out = Vec::with_capacity(128);
    let formatter = PythonJson {
        ascii: style.ascii,
        indent: style.indent,
        depth: 0,
        has_value: false,
    };
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    // Writing JSON values and string-keyed maps into a Vec cannot fail.
    if style.sort_keys {
        let tree = serde_json::to_value(value).expect("a value is always JSON");
        Sorted(&tree).serialize(&mut serializer)
    } else {
        value.serialize(&mut serializer)
    }
    .expect("a value is always JSON");
    String::from_utf8(out).expect("the formatter writes UTF-8 only")
}

/// `sort_keys=True`: Python sorts str keys by code point, which is the order of their UTF-8 bytes.
struct Sorted<'a>(&'a Value);

impl Serialize for Sorted<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Array(items) => s.collect_seq(items.iter().map(Sorted)),
            Value::Object(map) => {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                s.collect_map(entries.into_iter().map(|(key, item)| (key, Sorted(item))))
            }
            other => other.serialize(s),
        }
    }
}

/// `json.dumps`'s output format: `, ` between items, `: ` after a key, floats as `repr(float)`; with an indent,
/// `,` and a new line between items, and nothing inside an empty list or object.
struct PythonJson {
    ascii: bool,
    indent: Option<usize>,
    depth: usize,
    has_value: bool,
}

impl PythonJson {
    fn open<W: ?Sized + io::Write>(&mut self, w: &mut W, bracket: &[u8]) -> io::Result<()> {
        self.depth += 1;
        self.has_value = false;
        w.write_all(bracket)
    }

    fn close<W: ?Sized + io::Write>(&mut self, w: &mut W, bracket: &[u8]) -> io::Result<()> {
        self.depth -= 1;
        if self.has_value {
            self.newline(w)?;
        }
        w.write_all(bracket)
    }

    fn newline<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        if let Some(indent) = self.indent {
            w.write_all(b"\n")?;
            for _ in 0..indent * self.depth {
                w.write_all(b" ")?;
            }
        }
        Ok(())
    }

    fn item<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            // With an indent Python's item separator is "," followed by the new line.
            w.write_all(if self.indent.is_some() { b"," } else { b", " })?;
        }
        self.newline(w)
    }
}

impl Formatter for PythonJson {
    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.open(w, b"[")
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.close(w, b"]")
    }

    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.item(w, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.open(w, b"{")
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.close(w, b"}")
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.item(w, first)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(python_float(value).as_bytes())
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        w.write_all(python_float(f64::from(value)).as_bytes())
    }

    /// Python's `ensure_ascii`: everything outside `' '..='~'` is `\uXXXX` (a pair for characters above U+FFFF);
    /// without it the text is written as it is. The quote, the backslash and the control characters never
    /// reach here (`write_char_escape` has them), but U+007F does.
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        if !self.ascii {
            return w.write_all(fragment.as_bytes());
        }
        let mut plain = 0;
        for (at, ch) in fragment.char_indices() {
            if (' '..='~').contains(&ch) {
                continue;
            }
            w.write_all(&fragment.as_bytes()[plain..at])?;
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                write!(w, "\\u{unit:04x}")?;
            }
            plain = at + ch.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[plain..])
    }

    /// As serde_json writes them, which is as Python does: `\"`, `\\`, `\n`, `\r`, `\t`, `\b`, `\f`, and
    /// `\u00xx` in lowercase for the other control characters. `/` is not escaped.
    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        match escape {
            CharEscape::Quote => w.write_all(b"\\\""),
            CharEscape::ReverseSolidus => w.write_all(b"\\\\"),
            CharEscape::Solidus => w.write_all(b"/"),
            CharEscape::Backspace => w.write_all(b"\\b"),
            CharEscape::FormFeed => w.write_all(b"\\f"),
            CharEscape::LineFeed => w.write_all(b"\\n"),
            CharEscape::CarriageReturn => w.write_all(b"\\r"),
            CharEscape::Tab => w.write_all(b"\\t"),
            CharEscape::AsciiControl(byte) => write!(w, "\\u{byte:04x}"),
        }
    }
}

/// A number as Python writes it: an int in digits, a float as `repr(float)`.
pub fn number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        python_float(n.as_f64().unwrap_or(0.0))
    }
}

/// `repr(float)`: the shortest digits that read back as the same number; scientific notation
/// when the decimal exponent is below -4 or at least 16, with a sign and at least two exponent
/// digits; otherwise plain with a fractional part (`2.0`).
pub fn python_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    // Rust's `{:e}` gives the same shortest digits: "1.2345e-7"
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("{:e} always has an exponent");
    let exponent: i32 = exponent.parse().expect("{:e} writes a decimal exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if value < 0.0 { "-" } else { "" };
    if !(-4..16).contains(&exponent) {
        let mark = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{mantissa}e{mark}{:02}", exponent.abs());
    }
    if exponent < 0 {
        let zeros = "0".repeat((-exponent - 1) as usize);
        return format!("{sign}0.{zeros}{digits}");
    }
    let whole = exponent as usize + 1;
    if digits.len() <= whole {
        let zeros = "0".repeat(whole - digits.len());
        format!("{sign}{digits}{zeros}.0")
    } else {
        format!("{sign}{}.{}", &digits[..whole], &digits[whole..])
    }
}

/// `json.loads(text)` for stored columns. Python's decoder raises on what serde refuses; the
/// differences are listed at the top of this module.
pub fn loads(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// What `json.loads(text)` raises, in Python's own words (`"Expecting value: line 1 column 1
/// (char 0)"`), or `Ok` when Python reads it. Follows the C scanner of Python 3.12
/// (`Modules/_json.c`) and `json/decoder.py`; `ferb/store/salvage.py:72-80` shows the message to
/// the owner.
pub fn check(text: &str) -> Result<(), String> {
    let s: Vec<char> = text.chars().collect();
    let fail = |msg: &str, pos: usize| -> String {
        let line = s[..pos].iter().filter(|c| **c == '\n').count() + 1;
        let column = match s[..pos].iter().rposition(|c| *c == '\n') {
            Some(at) => pos - at,
            None => pos + 1,
        };
        format!("{msg}: line {line} column {column} (char {pos})")
    };
    if s.first() == Some(&'\u{feff}') {
        return Err(fail("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = skip_ws(&s, 0);
    let end = match scan_once(&s, start) {
        Ok(end) => end,
        Err(Scan::Stop(at)) => return Err(fail("Expecting value", at)),
        Err(Scan::Bad(msg, at)) => return Err(fail(msg, at)),
    };
    let end = skip_ws(&s, end);
    if end != s.len() {
        return Err(fail("Extra data", end));
    }
    Ok(())
}

enum Scan {
    /// Python's `StopIteration(idx)`: no value starts here.
    Stop(usize),
    Bad(&'static str, usize),
}

fn skip_ws(s: &[char], mut at: usize) -> usize {
    while at < s.len() && matches!(s[at], ' ' | '\t' | '\n' | '\r') {
        at += 1;
    }
    at
}

fn starts(s: &[char], at: usize, word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    s.len() >= at + word.len() && s[at..at + word.len()] == word[..]
}

/// `scan_once_unicode`: the index after the value starting at `at`.
fn scan_once(s: &[char], at: usize) -> Result<usize, Scan> {
    if at >= s.len() {
        return Err(Scan::Stop(at));
    }
    match s[at] {
        '"' => scan_string(s, at + 1),
        '{' => scan_object(s, at + 1),
        '[' => scan_array(s, at + 1),
        'n' if starts(s, at, "null") => Ok(at + 4),
        't' if starts(s, at, "true") => Ok(at + 4),
        'f' if starts(s, at, "false") => Ok(at + 5),
        'N' if starts(s, at, "NaN") => Ok(at + 3),
        'I' if starts(s, at, "Infinity") => Ok(at + 8),
        '-' if starts(s, at, "-Infinity") => Ok(at + 9),
        _ => scan_number(s, at),
    }
}

/// `_match_number_unicode`.
fn scan_number(s: &[char], start: usize) -> Result<usize, Scan> {
    let digit = |i: usize| i < s.len() && s[i].is_ascii_digit();
    let mut at = start;
    if s[at] == '-' {
        at += 1;
        if at >= s.len() {
            return Err(Scan::Stop(start));
        }
    }
    if ('1'..='9').contains(&s[at]) {
        while digit(at) {
            at += 1;
        }
    } else if s[at] == '0' {
        at += 1;
    } else {
        return Err(Scan::Stop(start));
    }
    if at < s.len() && s[at] == '.' && digit(at + 1) {
        at += 2;
        while digit(at) {
            at += 1;
        }
    }
    if at < s.len() && matches!(s[at], 'e' | 'E') {
        let mut e = at + 1;
        if e < s.len() && matches!(s[e], '-' | '+') {
            e += 1;
        }
        if digit(e) {
            while digit(e) {
                e += 1;
            }
            at = e;
        }
    }
    Ok(at)
}

/// `scanstring_unicode` with `strict`: `at` is the index after the opening quote.
fn scan_string(s: &[char], at: usize) -> Result<usize, Scan> {
    let len = s.len();
    let begin = at - 1;
    let mut end = at;
    loop {
        let mut c = '\0';
        let mut next = end;
        while next < len {
            c = s[next];
            if c == '"' || c == '\\' {
                break;
            }
            if c <= '\u{1f}' {
                return Err(Scan::Bad("Invalid control character at", next));
            }
            next += 1;
        }
        if !(c == '"' || c == '\\') {
            return Err(Scan::Bad("Unterminated string starting at", begin));
        }
        next += 1;
        if c == '"' {
            return Ok(next);
        }
        if next == len {
            return Err(Scan::Bad("Unterminated string starting at", begin));
        }
        c = s[next];
        if c != 'u' {
            end = next + 1;
            if !matches!(c, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(Scan::Bad("Invalid \\escape", end - 2));
            }
            continue;
        }
        next += 1;
        end = next + 4;
        if end >= len {
            return Err(Scan::Bad("Invalid \\uXXXX escape", next - 1));
        }
        let mut code = 0u32;
        while next < end {
            match s[next].to_digit(16).filter(|_| s[next].is_ascii()) {
                Some(d) => code = (code << 4) | d,
                None => return Err(Scan::Bad("Invalid \\uXXXX escape", end - 5)),
            }
            next += 1;
        }
        if (0xD800..0xDC00).contains(&code)
            && end + 6 < len
            && s[next] == '\\'
            && s[next + 1] == 'u'
        {
            next += 2;
            let pair_end = end + 6;
            let mut low = 0u32;
            while next < pair_end {
                match s[next].to_digit(16).filter(|_| s[next].is_ascii()) {
                    Some(d) => low = (low << 4) | d,
                    None => return Err(Scan::Bad("Invalid \\uXXXX escape", pair_end - 5)),
                }
                next += 1;
            }
            // A low surrogate joins the pair; anything else is read again as its own escape.
            if (0xDC00..0xE000).contains(&low) {
                end = pair_end;
            }
        }
    }
}

/// `_parse_object_unicode`: `at` is the index after `{`.
fn scan_object(s: &[char], at: usize) -> Result<usize, Scan> {
    let len = s.len();
    let mut at = skip_ws(s, at);
    if at >= len || s[at] != '}' {
        loop {
            if at >= len || s[at] != '"' {
                return Err(Scan::Bad(
                    "Expecting property name enclosed in double quotes",
                    at,
                ));
            }
            at = skip_ws(s, scan_string(s, at + 1)?);
            if at >= len || s[at] != ':' {
                return Err(Scan::Bad("Expecting ':' delimiter", at));
            }
            at = skip_ws(s, at + 1);
            at = skip_ws(s, scan_once(s, at)?);
            if at < len && s[at] == '}' {
                break;
            }
            if at >= len || s[at] != ',' {
                return Err(Scan::Bad("Expecting ',' delimiter", at));
            }
            at = skip_ws(s, at + 1);
        }
    }
    Ok(at + 1)
}

/// `_parse_array_unicode`: `at` is the index after `[`.
fn scan_array(s: &[char], at: usize) -> Result<usize, Scan> {
    let len = s.len();
    let mut at = skip_ws(s, at);
    if at >= len || s[at] != ']' {
        loop {
            at = skip_ws(s, scan_once(s, at)?);
            if at < len && s[at] == ']' {
                break;
            }
            if at >= len || s[at] != ',' {
                return Err(Scan::Bad("Expecting ',' delimiter", at));
            }
            at = skip_ws(s, at + 1);
        }
    }
    Ok(at + 1)
}

/// What `bytes.decode()` (UTF-8, strict) raises for `bytes`, in Python's words, or the text.
pub fn decode_utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|e| {
        let at = e.valid_up_to();
        let byte = bytes[at];
        // the bytes Python blames: those of the broken sequence, or all that are left
        let (count, why) = match e.error_len() {
            None => (bytes.len() - at, "unexpected end of data"),
            Some(n) if n == 1 && !(0xC2..=0xF4).contains(&byte) => (n, "invalid start byte"),
            Some(n) => (n, "invalid continuation byte"),
        };
        if count == 1 {
            format!("'utf-8' codec can't decode byte {byte:#04x} in position {at}: {why}")
        } else {
            format!(
                "'utf-8' codec can't decode bytes in position {at}-{}: {why}",
                at + count - 1
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_are_written_as_python_writes_them() {
        for (value, text) in [
            (1.5, "1.5"),
            (2.0, "2.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e-7, "1e-07"),
            (-2.5e-5, "-2.5e-05"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (123456789012345.0, "123456789012345.0"),
            (1234567890123456.0, "1234567890123456.0"),
            (12345678901234567.0, "1.2345678901234568e+16"),
            (6.02214076e23, "6.02214076e+23"),
            (1e300, "1e+300"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
            (100.0, "100.0"),
            (-12.75, "-12.75"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ] {
            assert_eq!(python_float(value), text);
        }
    }

    #[test]
    fn text_is_ascii_only_with_python_s_escapes() {
        let value =
            json!({"k\u{e9}": "a\u{7f}\u{1}\u{1f600}\"\\/\n", "list": [1, null, true], "o": {}});
        assert_eq!(
            dumps(&value),
            "{\"k\\u00e9\": \"a\\u007f\\u0001\\ud83d\\ude00\\\"\\\\/\\n\", \"list\": [1, null, true], \"o\": {}}"
        );
    }

    #[test]
    fn text_is_ascii_only_by_default_and_as_written_otherwise() {
        let value =
            json!({"k\u{e9}": "a\u{7f}\u{1}\u{1f600}\"\\/\n", "list": [1, null, true], "o": {}});
        assert_eq!(
            dumps(&value),
            "{\"k\\u00e9\": \"a\\u007f\\u0001\\ud83d\\ude00\\\"\\\\/\\n\", \"list\": [1, null, true], \"o\": {}}"
        );
        assert_eq!(
            dumps_utf8(&value),
            "{\"k\u{e9}\": \"a\u{7f}\\u0001\u{1f600}\\\"\\\\/\\n\", \"list\": [1, null, true], \"o\": {}}"
        );
    }

    #[test]
    fn sorted_keys_and_indent_are_python_s() {
        let value = json!({"b": [1, {"z": 1, "a": []}], "a": {}});
        let sorted = Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        };
        assert_eq!(
            dumps_with(&value, sorted),
            "{\"a\": {}, \"b\": [1, {\"a\": [], \"z\": 1}]}"
        );
        let indented = Style {
            ascii: false,
            sort_keys: false,
            indent: Some(2),
        };
        assert_eq!(
            dumps_with(&value, indented),
            "{\n  \"b\": [\n    1,\n    {\n      \"z\": 1,\n      \"a\": []\n    }\n  ],\n  \"a\": {}\n}"
        );
    }
}
