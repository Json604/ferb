//! Shared by the fixture replays: reading `rust/fixtures/sense/*.json`, the temporary folder written `<TMP>` in
//! them, and comparing values as the bytes Python writes.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sense")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture (run make.py)"))
        .expect("fixture JSON")
}

static TMP: OnceLock<PathBuf> = OnceLock::new();

/// This test binary's temporary folder (Python's TMP); HOME is `<TMP>/home` from the first call on.
pub fn tmp() -> &'static Path {
    TMP.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("ferb-sense-test-")
            .tempdir()
            .expect("a temporary folder")
            .keep();
        std::fs::create_dir_all(dir.join("home/.ferb")).expect("home");
        // SAFETY of the environment change: done once, before any test in this binary reads HOME.
        unsafe { std::env::set_var("HOME", dir.join("home")) };
        dir
    })
}

pub fn tmp_text() -> String {
    tmp().to_string_lossy().into_owned()
}

/// `<TMP>` in a fixture's text as this run's folder.
pub fn input(s: &str) -> String {
    s.replace("<TMP>", &tmp_text())
}

/// Every `<TMP>` in a value as this run's folder.
pub fn input_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::from(input(s)),
        Value::Array(a) => Value::Array(a.iter().map(input_value).collect()),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, v)| (input(k), input_value(v))).collect())
        }
        other => other.clone(),
    }
}

/// This run's folder (as given and resolved) written `<TMP>`, as make.py writes it.
pub fn output(s: &str) -> String {
    let real = std::fs::canonicalize(tmp())
        .expect("the folder")
        .to_string_lossy()
        .into_owned();
    s.replace(&real, "<TMP>").replace(&tmp_text(), "<TMP>")
}

pub fn output_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::from(output(s)),
        Value::Array(a) => Value::Array(a.iter().map(output_value).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (output(k), output_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

pub fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

/// Collects differences instead of stopping at the first, so one run shows them all.
#[derive(Default)]
pub struct Diffs {
    pub cases: usize,
    pub found: Vec<String>,
}

fn text(v: &Value) -> String {
    ferb_py::json::dumps_utf8(v)
}

impl Diffs {
    /// The two values written as Python's `json.dumps` writes them must be the same text (key order included).
    pub fn check(&mut self, what: &str, want: &Value, got: &Value) {
        self.cases += 1;
        let (w, g) = (text(want), text(&output_value(got)));
        if w != g {
            self.found
                .push(format!("{what}\n  python: {w}\n  rust:   {g}"));
        }
    }

    pub fn eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, want: T, got: T) {
        self.cases += 1;
        if want != got {
            self.found
                .push(format!("{what}\n  python: {want:?}\n  rust:   {got:?}"));
        }
    }

    pub fn finish(self, name: &str) {
        eprintln!(
            "{name}: {} comparisons, {} differ",
            self.cases,
            self.found.len()
        );
        if !self.found.is_empty() {
            let shown: Vec<&String> = self.found.iter().take(25).collect();
            panic!(
                "{name}: {} of {} differ:\n{}",
                self.found.len(),
                self.cases,
                shown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }
}

/// Python's `str(path)` written `<TMP>`.
pub fn out_path(p: &str) -> Value {
    Value::from(output(p))
}

/// `act/mac/browsers.py` `known` for the bundles these fixtures use (Launch Services' list left out, as make.py did).
pub fn known_browser(bundle: &str) -> bool {
    let b = ferb_sanitize::parity::casefold(bundle);
    [
        "com.google.chrome",
        "com.brave.browser",
        "com.apple.safari",
        "com.operasoftware",
        "com.vivaldi",
        "company.thebrowser",
        "org.chromium",
        "com.microsoft.edgemac",
        "org.mozilla.firefox",
    ]
    .iter()
    .any(|f| b == *f || b.starts_with(&format!("{f}.")))
}
