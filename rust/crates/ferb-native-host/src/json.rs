//! Checks that bytes are one JSON document as Python's `json.loads` reads it, without decoding it.
//!
//! The Python relay parses every message and writes it again (`ferb/native_host/main.py:30,35,56,61`). This relay
//! forwards the original bytes instead, so it only needs to know what Python would have accepted and where the
//! document starts and ends. Nothing here builds a string: a lone surrogate escape (`"\ud800"`), which Python accepts
//! and Rust's `str` cannot hold, passes through untouched, and so does every signed body (HAND-222).
//!
//! The grammar is CPython 3.12's C scanner with `strict=True` (`Modules/_json.c`): the four whitespace bytes,
//! `NaN`, `Infinity` and `-Infinity`, no control bytes inside strings, any `\uXXXX` including lone surrogates.
//! Python reads bytes as UTF-8, taking one leading byte-order mark (`json.detect_encoding`).

const BOM: &[u8] = b"\xef\xbb\xbf";

/// What the document's outermost value is.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    /// An object; `has_result` says one of its own keys is `result` (the hello reply, main.py:49).
    Object {
        has_result: bool,
    },
    Array,
    String,
    /// A number, `true`, `false`, `null`, `NaN` or an infinity.
    Scalar,
}

/// A checked document: `bytes[start..end]` is the value, without the mark and the whitespace around it.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Checked {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// `Some` exactly when Python's `json.loads(bytes)` returns a value (see the differences listed in main.rs).
pub fn check(bytes: &[u8]) -> Option<Checked> {
    let skip = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
    let b = &bytes[skip..];
    std::str::from_utf8(b).ok()?;
    let mut s = Scan { b, i: 0 };
    s.ws();
    let start = s.i;
    let kind = s.document()?;
    let end = s.i;
    s.ws();
    (s.i == b.len()).then_some(Checked {
        start: skip + start,
        end: skip + end,
        kind,
    })
}

struct Scan<'a> {
    b: &'a [u8],
    i: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, word: &[u8]) -> Option<()> {
        self.b[self.i..]
            .starts_with(word)
            .then(|| self.i += word.len())
    }

    /// One value, with an explicit stack: a megabyte of `[` must not overflow this thread's stack.
    fn document(&mut self) -> Option<Kind> {
        let mut open: Vec<bool> = Vec::new(); // true: an object, false: an array
        let mut has_result = false;
        let kind = match self.peek()? {
            b'{' => Kind::Object { has_result: false },
            b'[' => Kind::Array,
            b'"' => Kind::String,
            _ => Kind::Scalar,
        };
        'value: loop {
            // A value starts here (whitespace already skipped).
            match self.peek()? {
                b'{' | b'[' => {
                    let object = self.peek() == Some(b'{');
                    self.i += 1;
                    self.ws();
                    if self.peek()? == if object { b'}' } else { b']' } {
                        self.i += 1; // an empty container is a finished value
                    } else {
                        open.push(object);
                        if object {
                            has_result |= self.key()? && open.len() == 1;
                        }
                        continue 'value;
                    }
                }
                b'"' => {
                    self.string()?;
                }
                b'n' => self.eat(b"null")?,
                b't' => self.eat(b"true")?,
                b'f' => self.eat(b"false")?,
                b'N' => self.eat(b"NaN")?,
                b'I' => self.eat(b"Infinity")?,
                _ => self.number()?,
            }
            // A value just ended: close containers, or move to the next member.
            loop {
                let Some(&object) = open.last() else {
                    return Some(match kind {
                        Kind::Object { .. } => Kind::Object { has_result },
                        other => other,
                    });
                };
                self.ws();
                match (self.peek()?, object) {
                    (b',', _) => {
                        self.i += 1;
                        self.ws();
                        if object {
                            has_result |= self.key()? && open.len() == 1;
                        }
                        continue 'value;
                    }
                    (b'}', true) | (b']', false) => {
                        self.i += 1;
                        open.pop();
                    }
                    _ => return None,
                }
            }
        }
    }

    /// An object key, its colon and the whitespace after it; true when the key reads `result`.
    fn key(&mut self) -> Option<bool> {
        let (from, to) = self.string()?;
        let is_result = reads_result(&self.b[from..to]);
        self.ws();
        self.eat(b":")?;
        self.ws();
        Some(is_result)
    }

    /// A string; returns the span between its quotes.
    fn string(&mut self) -> Option<(usize, usize)> {
        self.eat(b"\"")?;
        let from = self.i;
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                b'"' => return Some((from, self.i - 1)),
                b'\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    match e {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => {
                            let hex = self.b.get(self.i..self.i + 4)?;
                            if !hex.iter().all(u8::is_ascii_hexdigit) {
                                return None;
                            }
                            self.i += 4;
                        }
                        _ => return None,
                    }
                }
                0..=0x1f => return None, // strict: no raw control byte, so no raw newline, inside a string
                _ => {}
            }
        }
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?` or `-Infinity`. A dangling `.` or `e` is left for the
    /// caller, where it is a syntax error, as in `_match_number_unicode`.
    fn number(&mut self) -> Option<()> {
        if self.peek() == Some(b'-') {
            self.i += 1;
            if self.peek() == Some(b'I') {
                return self.eat(b"Infinity");
            }
        }
        match self.peek()? {
            b'0' => self.i += 1,
            b'1'..=b'9' => self.digits(),
            _ => return None,
        }
        if self.peek() == Some(b'.') && self.b.get(self.i + 1).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(self.b.get(self.i + 1), Some(b'+' | b'-')));
            if self
                .b
                .get(self.i + 1 + sign)
                .is_some_and(u8::is_ascii_digit)
            {
                self.i += 1 + sign;
                self.digits();
            }
        }
        Some(())
    }

    fn digits(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
    }
}

/// Whether an already checked string body decodes to `result` (its letters may be written as escapes).
fn reads_result(raw: &[u8]) -> bool {
    let mut decoded = Vec::with_capacity(6);
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'\\' {
            decoded.push(raw[i]);
            i += 1;
        } else if raw[i + 1] == b'u' {
            let unit = std::str::from_utf8(&raw[i + 2..i + 6])
                .ok()
                .and_then(|h| u16::from_str_radix(h, 16).ok());
            match unit {
                Some(u) if u < 0x80 => decoded.push(u as u8),
                _ => return false,
            }
            i += 6;
        } else {
            return false; // \" \\ \/ \b \f \n \r \t: none is a letter of `result`
        }
    }
    decoded == b"result"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(s: &[u8]) -> Option<Kind> {
        check(s).map(|c| c.kind)
    }

    #[test]
    fn accepts_what_python_accepts() {
        for ok in [
            &br#"{}"#[..],
            br#"{"a": [1, -0, 1.5e+3, 1E2, 0.0, NaN, Infinity, -Infinity, true, false, null]}"#,
            br#"{"a": "\ud800", "\udc00": "\"\\\/\b\f\n\r\t\u00e9"}"#,
            b"\xef\xbb\xbf{ }",
            b" \t\r\n{\n\"a\"\t:\r\n[ ]\n}\n",
            "{\"é\": \"日本 😀 \u{7f}\"}".as_bytes(),
            br#"{"a": 1, "a": 2}"#,
        ] {
            assert!(
                matches!(kind(ok), Some(Kind::Object { .. })),
                "{:?}",
                String::from_utf8_lossy(ok)
            );
        }
        assert_eq!(kind(b"[1, [2, {}]]"), Some(Kind::Array));
        assert_eq!(kind(b"\"s\""), Some(Kind::String));
        for scalar in [&b"0"[..], b"-1.5e-7", b"null", b"NaN", b"-Infinity"] {
            assert_eq!(kind(scalar), Some(Kind::Scalar));
        }
    }

    #[test]
    fn refuses_what_python_refuses() {
        for bad in [
            &b""[..],
            b" ",
            b"{",
            br#"{"a":"#,
            br#"{"a": 1,}"#,
            b"[1,]",
            b"[1 2]",
            br#"{'a': 1}"#,
            br#"{a: 1}"#,
            br#"{"a" 1}"#,
            b"01",
            b"1.",
            b"1e",
            b"-",
            b"+1",
            b".5",
            b"-NaN",
            b"nul",
            b"True",
            b"{} {}",
            b"{}x",
            b"\"a\nb\"",
            b"\"a\x00b\"",
            br#""\x""#,
            br#""\u12G4""#,
            br#""\u12""#,
            b"\"open",
            b"\"\xff\"",
            b"\xed\xa0\x80",
            b"\xef\xbb\xbf\xef\xbb\xbf{}",
            b"{]",
            b"[}",
        ] {
            assert_eq!(check(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let mut deep = vec![b'['; 500_000];
        deep.extend(vec![b']'; 500_000]);
        assert_eq!(kind(&deep), Some(Kind::Array));
        deep.pop();
        assert_eq!(check(&deep), None);
    }

    #[test]
    fn the_span_leaves_out_the_mark_and_the_whitespace() {
        let c = check(b"\xef\xbb\xbf \n{\"a\": 1}\r\n").unwrap();
        assert_eq!((c.start, c.end), (5, 13));
    }

    #[test]
    fn result_is_looked_for_among_the_outer_keys_only() {
        let has = |s: &[u8]| kind(s) == Some(Kind::Object { has_result: true });
        assert!(has(br#"{"id": 1, "result": {"role": "native_host"}}"#));
        assert!(has(br#"{"r\u0065sul\u0074": null}"#));
        assert!(has(br#"{"a": {"b": [1]}, "result": 1}"#));
        assert!(!has(br#"{"error": {"result": 1}}"#));
        assert!(!has(br#"{"a": "result"}"#));
        assert!(!has(br#"{"results": 1, "resul": 2, "Result": 3}"#));
    }
}
