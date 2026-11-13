//! Shared by the fixture tests: the fixtures made by rust/fixtures/browser/make.py.
#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;

pub fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/browser")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

pub fn b64d(s: &str) -> Vec<u8> {
    ferb_identity::py::b64decode(s).expect("base64")
}

/// The fixtures' random source: SHA-256 of "ferb-fixture-<i>" for i = 1, 2, ..., cut to the size asked
/// (make.py `counter_bytes`).
pub struct Counter(pub std::sync::Mutex<u64>);

impl Counter {
    pub fn new() -> Counter {
        Counter(std::sync::Mutex::new(0))
    }
}

impl ferb_browser::pairing::Random for Counter {
    fn bytes(&self, n: usize) -> Vec<u8> {
        use sha2::Digest;
        let mut out = vec![];
        let mut i = self.0.lock().unwrap();
        while out.len() < n {
            *i += 1;
            out.extend_from_slice(&sha2::Sha256::digest(
                format!("ferb-fixture-{}", *i).as_bytes(),
            ));
        }
        out.truncate(n);
        out
    }
}
