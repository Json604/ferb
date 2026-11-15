//! Shared by the fixture tests: the fixture folder, a temporary root under /tmp (whose parents are the same on every
//! Mac, as make.py's), and the @ROOT@ / @REAL@ placeholders.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/shell")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

pub struct Root {
    pub dir: tempfile::TempDir,
    pub root: String,
    pub real: String,
}

impl Root {
    pub fn new() -> Root {
        let dir = tempfile::Builder::new()
            .prefix("ferbshell")
            .tempdir_in("/tmp")
            .expect("a temporary folder");
        let root = dir.path().to_string_lossy().into_owned();
        let real = ferb_shell::pyos::realpath(dir.path())
            .to_string_lossy()
            .into_owned();
        assert!(
            root.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_-".contains(c)),
            "{root}"
        );
        Root { dir, root, real }
    }

    /// A fixture text with the placeholders replaced by this root.
    pub fn sub(&self, s: &str) -> String {
        s.replace("@REAL@", &self.real)
            .replace("@ROOT@", &self.root)
    }

    pub fn path(&self, s: &str) -> PathBuf {
        PathBuf::from(self.sub(s))
    }

    /// Rebuild make.py's folder layout ops under this root.
    pub fn build(&self, ops: &[Value]) {
        for op in ops {
            let op: Vec<&str> = op
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let target = Path::new(&self.root).join(op[1]);
            match op[0] {
                "dir" => std::fs::create_dir_all(&target).unwrap(),
                "file" => {
                    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                    std::fs::write(&target, op[2]).unwrap();
                }
                "link" => {
                    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink(op[2].replace("@ROOT@", &self.root), &target)
                        .unwrap();
                }
                other => panic!("unknown op {other}"),
            }
        }
    }
}

pub fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

/// Counts differences instead of stopping at the first, so a report says how many cases differ.
#[derive(Default)]
pub struct Tally {
    pub cases: usize,
    pub failures: Vec<String>,
}

impl Tally {
    pub fn check<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
        self.cases += 1;
        if got != want {
            self.failures
                .push(format!("{what}:\n  got:  {got:?}\n  want: {want:?}"));
        }
    }

    pub fn done(self, name: &str) {
        eprintln!(
            "{name}: {} cases, {} differ",
            self.cases,
            self.failures.len()
        );
        if !self.failures.is_empty() {
            let shown: Vec<&String> = self.failures.iter().take(12).collect();
            panic!(
                "{} of {} {name} cases differ:\n{}",
                self.failures.len(),
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

pub fn map_of(v: &Value) -> HashMap<String, Value> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
