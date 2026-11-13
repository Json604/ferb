//! What the identity tests share (also used by `ferb-platform-macos/tests/identity.rs`): the fixture
//! `rust/fixtures/identity/make.py` wrote, and the seams answered as Python's own code answered them.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use ferb_identity::{owner_secrets::Places, py};
use serde_json::Value;

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/identity")
}

pub fn cases() -> &'static Value {
    static CASES: OnceLock<Value> = OnceLock::new();
    CASES.get_or_init(|| {
        serde_json::from_slice(&fs::read(fixtures().join("cases.json")).unwrap()).unwrap()
    })
}

pub fn unb64(value: &Value) -> Vec<u8> {
    py::b64decode(value.as_str().unwrap()).unwrap()
}

pub fn unb64_text(value: &Value) -> String {
    String::from_utf8(unb64(value)).unwrap()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

/// Equal as Python sees them, key order included.
pub fn same(got: &Value, want: &Value, what: &str) {
    assert_eq!(py::dumps(got), py::dumps(want), "{what}");
}

// ---------- the seams, answered as Python's own code answered (the fixture's tables) ----------

pub struct Recorded;

impl Places for Recorded {
    fn is_browser(&self, bundle_id: &str) -> bool {
        cases()["seams"]["is_browser"][bundle_id]
            .as_bool()
            .unwrap_or_else(|| panic!("is_browser({bundle_id:?}) was never asked in Python"))
    }

    fn url_host(&self, url: &str) -> Option<String> {
        match cases()["seams"]["url_host"].get(url) {
            Some(Value::String(host)) => Some(host.clone()),
            Some(_) => None,
            None => panic!("urls.parse({url:?}) was never asked in Python"),
        }
    }
}
