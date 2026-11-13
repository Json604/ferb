//! Shared by the fixture tests: reading `rust/fixtures/memory/*.json`, float bits, base64, the fixture's fake
//! redactor, and a model folder written from the fixture's bytes.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/memory")
        .join(name);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

pub fn bits(x: f64) -> String {
    format!("{:016x}", x.to_bits())
}

pub fn unbits(s: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(s, 16).unwrap())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn b64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u32, 0);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
            acc &= (1 << n) - 1;
        }
    }
    out
}

pub fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

pub fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

pub fn i8s(v: &Value) -> Vec<i8> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap() as i8)
        .collect()
}

/// The fixture's redactor: every word starting "SECRET-" masked (make.py `fake_redact`).
pub fn fake_redact(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let pat: Vec<char> = "SECRET-".chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&pat) {
            let mut k = i + pat.len();
            while k < chars.len() && (python_isalnum(chars[k]) || chars[k] == '-') {
                k += 1;
            }
            out.push_str("[redacted]");
            i = k;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn python_isalnum(c: char) -> bool {
    // the fixture's secrets are ASCII; non-ASCII letters never follow "SECRET-" there
    c.is_ascii_alphanumeric() || (!c.is_ascii() && c.is_alphanumeric())
}

/// Write a model's files (`{name: base64}`) into `root`.
pub fn write_model(root: &Path, files: &Value) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    for (name, data) in files.as_object().unwrap() {
        fs::write(root.join(name), b64(s(data))).unwrap();
    }
    root.to_path_buf()
}
