//! Shared replay helpers: the fixture files' trees, a scripted runner that answers each command as the Python run's
//! fake did (and makes the files that fake made), and a stepped clock.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ferb_device::install::{Clock, Output, Runner};
use serde_json::{json, Value};

pub const UID: u32 = 4321;

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/device")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("fixture json")
}

/// A fixture string with this run's uid.
pub fn subst(s: &str) -> String {
    s.replace("gui/$UID", &format!("gui/{UID}"))
}

/// `.ferb-lock-<name>` written as `.ferb-lock-*` (tempfile's random names).
pub fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find(".ferb-lock-") {
        out.push_str(&rest[..at + ".ferb-lock-".len()]);
        let tail = &rest[at + ".ferb-lock-".len()..];
        let end = tail.find('/').unwrap_or(tail.len());
        out.push('*');
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

// ---------- trees ----------

pub fn entry_bytes(e: &Value) -> Vec<u8> {
    if let Some(t) = e.get("text").and_then(Value::as_str) {
        return t.as_bytes().to_vec();
    }
    hex::decode(e["hex"].as_str().unwrap()).unwrap()
}

fn content(data: &[u8]) -> Value {
    match std::str::from_utf8(data) {
        Ok(t) => json!({"text": t}),
        Err(_) => json!({"hex": hex::encode(data)}),
    }
}

/// Every entry under `root`, as make.py's `snap` writes it.
pub fn snap(root: &Path) -> BTreeMap<String, Value> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Value>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            let mode = meta.permissions().mode() & 0o7777;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&p).unwrap();
                out.insert(
                    rel,
                    json!({"kind": "link", "target": target.to_string_lossy()}),
                );
            } else if meta.is_dir() {
                out.insert(rel, json!({"kind": "dir", "mode": mode}));
                walk(root, &p, out);
            } else {
                let mut v = json!({"kind": "file", "mode": mode});
                let c = content(&std::fs::read(&p).unwrap());
                for (k, x) in c.as_object().unwrap() {
                    v[k] = x.clone();
                }
                out.insert(rel, v);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub fn changes(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for (k, v) in after {
        if before.get(k) != Some(v) {
            out.insert(norm(k), normalize_entry(v));
        }
    }
    for k in before.keys() {
        if !after.contains_key(k) {
            out.insert(norm(k), json!({"kind": "gone"}));
        }
    }
    out
}

fn normalize_entry(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(t) = v.get("target").and_then(Value::as_str) {
        v["target"] = json!(norm(t));
    }
    if let Some(t) = v.get("text").and_then(Value::as_str) {
        v["text"] = json!(norm(&subst(t)));
    }
    v
}

/// The fixture's list of changes keyed by path (normalized like `changes`).
pub fn expected_changes(list: &Value) -> BTreeMap<String, Value> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let mut e = e.clone();
            let path = e["path"].as_str().unwrap().to_string();
            e.as_object_mut().unwrap().remove("path");
            (norm(&path), normalize_entry(&e))
        })
        .collect()
}

fn safe(root: &Path, rel: &str) -> PathBuf {
    assert!(
        !rel.starts_with('/') && !rel.split('/').any(|p| p == ".."),
        "{rel}"
    );
    root.join(rel)
}

fn remove_any(p: &Path) {
    if let Ok(m) = std::fs::symlink_metadata(p) {
        if m.is_dir() && !m.file_type().is_symlink() {
            std::fs::remove_dir_all(p).unwrap();
        } else {
            std::fs::remove_file(p).unwrap();
        }
    }
}

/// Applies a list of changes to the tree under `root` (gone entries first, deepest first; then the rest in order).
/// `rename` maps fixture names of tempfile folders to this run's.
pub fn apply(root: &Path, list: &Value, rename: &[(String, String)]) {
    let fix = |s: &str| {
        let mut s = subst(s);
        for (a, b) in rename {
            s = s.replace(a, b);
        }
        s
    };
    let entries = list.as_array().unwrap();
    let mut gone: Vec<&Value> = entries.iter().filter(|e| e["kind"] == "gone").collect();
    gone.sort_by(|a, b| b["path"].as_str().cmp(&a["path"].as_str()));
    for e in gone {
        remove_any(&safe(root, &fix(e["path"].as_str().unwrap())));
    }
    for e in entries.iter().filter(|e| e["kind"] != "gone") {
        let p = safe(root, &fix(e["path"].as_str().unwrap()));
        match e["kind"].as_str().unwrap() {
            "dir" => {
                if std::fs::symlink_metadata(&p)
                    .is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink())
                {
                    remove_any(&p);
                }
                std::fs::create_dir_all(&p).unwrap();
                std::fs::set_permissions(
                    &p,
                    std::fs::Permissions::from_mode(e["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
            "file" => {
                if std::fs::symlink_metadata(&p)
                    .is_ok_and(|m| m.is_dir() || m.file_type().is_symlink())
                {
                    remove_any(&p);
                }
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                let mut data = entry_bytes(e);
                if e.get("text").is_some() {
                    data = fix(std::str::from_utf8(&data).unwrap()).into_bytes();
                }
                std::fs::write(&p, data).unwrap();
                std::fs::set_permissions(
                    &p,
                    std::fs::Permissions::from_mode(e["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
            "link" => {
                remove_any(&p);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(fix(e["target"].as_str().unwrap()), &p).unwrap();
            }
            other => panic!("unknown entry kind {other}"),
        }
    }
}

/// A fresh root holding the fixture's starting files.
pub fn materialize(root: &Path, initial: &Value) {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root).unwrap();
    apply(root, initial, &[]);
}

// ---------- the scripted world ----------

/// The events the Python run recorded for one step, consumed in order by the seams.
#[derive(Default)]
pub struct Script {
    pub events: VecDeque<Value>,
    pub root: PathBuf,
    pub context: String,
}

pub type Shared = Rc<RefCell<Script>>;

impl Script {
    /// The next event, which must be of `kind`.
    pub fn next(&mut self, kind: &str, what: &str) -> Value {
        let ev = self
            .events
            .pop_front()
            .unwrap_or_else(|| panic!("{}: Rust made an extra {kind} call: {what}", self.context));
        assert_eq!(
            ev["kind"], kind,
            "{}: Python's next event was {ev}, Rust made {kind} {what}",
            self.context
        );
        ev
    }
}

/// Lock-folder renames between the fixture's argv and this run's.
pub fn renames(expected: &[String], actual: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (e, a) in expected.iter().zip(actual) {
        if e != a && norm(e) == norm(a) {
            let pick = |s: &str| {
                let at = s.find(".ferb-lock-").unwrap();
                let tail = &s[at..];
                tail[..tail.find('/').unwrap_or(tail.len())].to_string()
            };
            out.push((pick(e), pick(a)));
        }
    }
    out
}

/// A runner that answers as the recorded fake did.
pub struct ScriptRunner(pub Shared, pub &'static str);

impl Runner for ScriptRunner {
    fn run(&mut self, argv: &[String]) -> Output {
        let mut s = self.0.borrow_mut();
        let ev = s.next(self.1, &format!("{argv:?}"));
        let expected: Vec<String> = ev["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| subst(a.as_str().unwrap()))
            .collect();
        let exp_n: Vec<String> = expected.iter().map(|a| norm(a)).collect();
        let act_n: Vec<String> = argv.iter().map(|a| norm(a)).collect();
        assert_eq!(act_n, exp_n, "{}: command differs", s.context);
        let rename = renames(&expected, argv);
        let root = s.root.clone();
        apply(&root, &ev["effects"], &rename);
        Output {
            code: ev["rc"].as_i64().unwrap() as i32,
            stdout: subst(ev["stdout"].as_str().unwrap()),
            stderr: subst(ev["stderr"].as_str().unwrap()),
        }
    }
}

/// tests/conftest.py SteppedTime.
#[derive(Clone, Default)]
pub struct Stepped(pub Rc<RefCell<f64>>);

impl Clock for Stepped {
    fn monotonic(&mut self) -> f64 {
        *self.0.borrow()
    }
    fn sleep(&mut self, seconds: f64) {
        *self.0.borrow_mut() += seconds;
    }
}
