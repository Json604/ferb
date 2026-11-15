//! The pieces of Python's own behavior the identity code leans on: `json.dumps`, `str.split` and
//! `str.strip` whitespace, truthiness, `repr` of a string, base64. Each is compared with Python's
//! output in `tests/parity.rs` (decision: ENG-3).

use serde_json::Value;

/// `str.isspace()` for one character, `text.strip()`, `' '.join(text.split())` and `bool(value)`
/// for a value `json.loads` made: `ferb-py`'s, shared with every port.
pub use ferb_py::value::{is_py_space as is_space, squeeze as join_split, strip, truthy};

/// `json.dumps(value)` with Python's defaults: `", "` and `": "`, everything outside ASCII as
/// `\uXXXX` (astral characters as a surrogate pair), keys in the map's own order. Appends to
/// `out`. Integers that fit 64 bits are written as Python writes them; other numbers are written
/// as serde_json does (not compared with Python: FERB's own records hold none).
pub fn dumps_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => string_into(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                dumps_into(item, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            out.push(b'{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                string_into(key, out);
                out.extend_from_slice(b": ");
                dumps_into(item, out);
            }
            out.push(b'}');
        }
    }
}

/// `json.dumps(value)` as text.
pub fn dumps(value: &Value) -> String {
    let mut out = Vec::new();
    dumps_into(value, &mut out);
    String::from_utf8(out).expect("json.dumps output is ASCII")
}

/// The most bytes `string_into` writes for `text`: two quotes and six bytes per UTF-8 byte
/// (`\u00XX` for a control byte is the worst case; a four-byte character becomes twelve).
pub fn string_bound(text: &str) -> usize {
    2 + 6 * text.len()
}

/// One JSON string as `json.dumps` writes it (`json.encoder.py_encode_basestring_ascii`).
pub fn string_into(text: &str, out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let unit = |out: &mut Vec<u8>, u: u16| {
        out.extend_from_slice(b"\\u");
        for shift in [12, 8, 4, 0] {
            out.push(HEX[usize::from((u >> shift) & 0xf)]);
        }
    };
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            ' '..='~' => out.push(c as u8),
            _ => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    unit(out, *u);
                }
            }
        }
    }
    out.push(b'"');
}

/// `repr(text)` for a string: single quotes unless the text has one and no double quote; `\\`,
/// `\n`, `\r`, `\t` and the quote escaped; other ASCII controls as `\xNN`. Characters outside
/// ASCII are written as they are, except controls (`\xNN`, `\uNNNN`): Python also escapes the
/// other characters its tables call unprintable (unassigned, format, separators), which is not
/// reproduced here.
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
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.b64encode(data).decode()`.
pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(B64[(n >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `secrets.token_urlsafe`'s alphabet: URL-safe base64 with the padding removed.
pub fn b64encode_urlsafe_nopad(data: &[u8]) -> String {
    b64encode(data)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// Standard base64 with padding, strictly: `None` for anything else. (Python's `b64decode` skips
/// characters outside the alphabet; nothing FERB makes holds any.)
pub fn b64decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let last = i + 1 == bytes.len() / 4;
        let pad = chunk.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for b in &chunk[..4 - pad] {
            n = n << 6 | B64.iter().position(|a| a == b)? as u32;
        }
        n <<= 6 * pad as u32;
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0, 255, 254, 3, 2]] {
            assert_eq!(b64decode(&b64encode(data)).as_deref(), Some(data));
        }
        assert_eq!(b64encode(b"ab"), "YWI=");
        assert_eq!(b64decode("YW=I"), None);
        assert_eq!(b64decode("YWI"), None);
        assert_eq!(b64encode_urlsafe_nopad(&[0xfb, 0xff]), "-_8");
    }
}
