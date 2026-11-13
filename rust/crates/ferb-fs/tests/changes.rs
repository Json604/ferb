//! The change record against what the Python one did (decision: ENG-3).
//! `rust/fixtures/fs/make_changes.py` ran `ferb/act/fs/changes.py` through scripted scenarios on invented file trees
//! (the cases of tests/test_changes*.py) and wrote every call's output; each test here replays the same steps on its
//! own temporary folder and must get the same outputs. Races run at the same moments, through `ferb_fs::hooks`.
//! One deliberate difference is named where it is compared: `named()`'s order for a word holding an embedded path.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use ferb_fs::changes::{self, Change, Changes, PdfText, Text};
use ferb_fs::difflib::{splitlines, str_ratio, unified_diff, SequenceMatcher};
use ferb_fs::{hooks, identity, pypath as pp, Closed};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const NOW: i64 = 1_600_000_000_000_000_000;

fn fixture() -> &'static Value {
    static F: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        // the fixture's local times are UTC (`TZ=UTC` when it was made): `_when` reads the local zone
        std::env::set_var("TZ", "UTC");
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fs/changes.json");
        serde_json::from_str(&fs::read_to_string(path).expect("run make_changes.py")).expect("json")
    })
}

/// PDFKit's answers Python recorded, by the bytes' SHA-256.
struct Recorded(HashMap<String, Option<Text>>);

impl PdfText for Recorded {
    fn text(&self, data: &[u8]) -> Option<Text> {
        let key = hex::encode(Sha256::digest(data));
        self.0
            .get(&key)
            .cloned()
            .expect("a PDF the fixture recorded")
    }
}

fn recorded_pdf() -> Arc<dyn PdfText> {
    let mut map = HashMap::new();
    for (k, v) in fixture()["pdf"].as_object().unwrap() {
        let got = v.as_array().map(|a| {
            let notes = a[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| {
                    (
                        n[0].as_i64().unwrap(),
                        n[1].as_str().unwrap().to_string(),
                        n[2].as_str().unwrap().to_string(),
                    )
                })
                .collect();
            (a[0].as_str().unwrap().to_string(), notes)
        });
        map.insert(k.clone(), got);
    }
    Arc::new(Recorded(map))
}

/// `Protection(home, ferb, never_read, [], []).read_denied(path) is not None`, from the identity half the crate
/// ports (protection.py `_forms`, `_state`, `_hit`; no app bundle, no other app's containers in these trees).
struct Protection {
    states: Vec<(String, String, String)>,
    entries: Vec<Vec<String>>,
    workspace: String,
    home: String,
}

impl Protection {
    fn new(home: &str, ferb: &str, never_read: &[&str]) -> Protection {
        let ferb_home = identity::real(ferb, home);
        let mut homes = vec![ferb_home.clone(), identity::typed(ferb)];
        homes.extend(identity::kernel(ferb));
        let mut states = Vec::new();
        for h in homes {
            let t = (
                h.clone(),
                pp::join(&h, "workspace"),
                pp::join_all(&h, ["workspace", "uploads"]),
            );
            if !states.contains(&t) {
                states.push(t);
            }
        }
        let entries = never_read
            .iter()
            .map(|e| {
                let raw = if *e == "~" || e.starts_with("~/") {
                    pp::join(home, e.get(2..).unwrap_or(""))
                } else {
                    pp::join(home, e)
                };
                let real = identity::real(&raw, home);
                let mut roots = vec![real.clone(), identity::typed(&raw)];
                roots.extend(identity::firmlinked(&real).or_else(|| identity::firmlinked(&raw)));
                roots
            })
            .collect();
        Protection {
            states,
            entries,
            workspace: pp::join(&ferb_home, "workspace"),
            home: home.into(),
        }
    }

    fn read_denied(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        let path = if pp::is_abs(&path) {
            pp::norm(&path)
        } else {
            pp::join(&self.workspace, &path)
        };
        let forms = identity::forms(&path, &self.home);
        if forms.iter().any(|p| identity::unmapped(p)) {
            return true;
        }
        let state = forms.iter().any(|p| {
            self.states.iter().any(|(h, ws, up)| {
                (identity::under(p, h) && !identity::under(p, ws)) || identity::under(p, up)
            })
        });
        state
            || self.entries.iter().any(|roots| {
                forms
                    .iter()
                    .any(|p| roots.iter().any(|r| identity::under(p, r)))
            })
    }
}

struct Scene {
    root: String,
    home: String,
    cfg: Value,
    pdf: Arc<dyn PdfText>,
}

impl Scene {
    fn at(&self, rel: &str) -> String {
        pp::join(&self.root, rel)
    }
    fn sub(&self, s: &str) -> String {
        s.replace("$ROOT", &self.root)
    }
    fn store(&self) -> Changes {
        let closed: Option<Closed> = match &self.cfg["closed"] {
            Value::String(s) if s == "protection" => {
                let reads: Vec<String> = self.cfg["never_read"]
                    .as_array()
                    .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                    .unwrap_or_else(|| vec!["~/.ssh".into()]);
                let refs: Vec<&str> = reads.iter().map(String::as_str).collect();
                let p = Protection::new(
                    &self.home,
                    &self.at(self.cfg["ferb"].as_str().unwrap()),
                    &refs,
                );
                Some(Arc::new(move |path: &Path| p.read_denied(path)))
            }
            Value::Object(m) => {
                let part = m["part"].as_str().unwrap().to_string();
                Some(Arc::new(move |path: &Path| {
                    pp::tail(&path.to_string_lossy()).contains(&part.as_str())
                }))
            }
            _ => None,
        };
        let root = self.at(self.cfg["root"].as_str().unwrap_or("changes"));
        Changes::new(&root, closed, &self.home).with_pdf(self.pdf.clone())
    }
}

fn norm_text(text: &str, root: &str) -> String {
    let text = text.replace(root, "$ROOT");
    static RX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let rx = RX.get_or_init(|| {
        regex::Regex::new(r", modified (\d{4})-\d\d-\d\dT\d\d:\d\d:\d\d[+-]\d\d:\d\d(?::\d\d)?")
            .unwrap()
    });
    rx.replace_all(&text, |c: &regex::Captures| {
        if c[1].parse::<i32>().unwrap() >= 2020 {
            ", modified <now>".to_string()
        } else {
            c[0].to_string()
        }
    })
    .into_owned()
}

fn norm_value(v: &Value, root: &str) -> Value {
    match v {
        Value::String(s) => Value::String(norm_text(s, root)),
        Value::Array(a) => Value::Array(a.iter().map(|x| norm_value(x, root)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (norm_text(k, root), norm_value(x, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn norm_change(c: &Change, root: &str) -> Value {
    let mut d = norm_value(&c.to_json(), root);
    if d["mtime_ns"].as_i64().is_some_and(|n| n >= NOW) {
        d["mtime_ns"] = json!("<now>");
    }
    d
}

fn norm_index(text: Option<&str>, root: &str) -> Value {
    let Some(text) = text else { return Value::Null };
    let mut data: Value = serde_json::from_str(text).unwrap();
    for f in data["files"].as_object_mut().unwrap().values_mut() {
        f["ino"] = json!("<id>");
        f["dev"] = json!("<id>");
        if f["mtime_ns"].as_i64().unwrap() >= NOW {
            f["mtime_ns"] = json!("<now>");
        }
    }
    for listing in data["folders"].as_object_mut().unwrap().values_mut() {
        for e in listing.as_object_mut().unwrap().values_mut() {
            let a = e.as_array().unwrap().clone();
            let t = a[2].as_i64().unwrap();
            *e = json!([
                a[0],
                a[1],
                if t >= NOW { json!("<now>") } else { json!(t) },
                "<id>"
            ]);
        }
    }
    Value::String(norm_text(&ferb_py::json::dumps(&data), root))
}

fn snapshot(top: &str, root: &str) -> Value {
    let mut out = Vec::new();
    fn walk(dir: &str, top: &str, root: &str, out: &mut Vec<Value>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut subdirs = Vec::new();
        for name in &names {
            let p = pp::join(dir, name);
            let rel = pp::relative_to(&p, top);
            let md = fs::symlink_metadata(&p).unwrap();
            let mode = md.permissions().mode() & 0o7777;
            if md.file_type().is_symlink() {
                out.push(json!([
                    rel,
                    "link",
                    norm_text(&fs::read_link(&p).unwrap().to_string_lossy(), root)
                ]));
                if fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
                    // os.walk lists a link to a folder among the folders but never walks it
                }
            } else if md.is_dir() {
                out.push(json!([rel, "dir", mode]));
                subdirs.push(p);
            } else if md.is_file() {
                let mut data = fs::read(&p).unwrap();
                let mut size = md.len() as usize;
                if name == changes::INDEX {
                    let n = norm_index(Some(std::str::from_utf8(&data).unwrap()), root);
                    data = n.as_str().unwrap().as_bytes().to_vec();
                    size = data.len();
                }
                use std::os::unix::fs::MetadataExt;
                out.push(json!([
                    rel,
                    "file",
                    size,
                    hex::encode(Sha256::digest(&data)),
                    md.nlink(),
                    mode
                ]));
            } else {
                out.push(json!([rel, "other"]));
            }
        }
        for d in subdirs {
            walk(&d, top, root, out);
        }
    }
    walk(top, top, root, &mut out);
    Value::Array(out)
}

fn cstr(p: &str) -> std::ffi::CString {
    std::ffi::CString::new(p).unwrap()
}

fn fs_op(sc: &Scene, step: &Value) {
    let op = step["op"].as_str().unwrap();
    let p = step["path"].as_str().map(|r| sc.at(r)).unwrap_or_default();
    let s = |k: &str| step[k].as_str().unwrap().to_string();
    match op {
        "mkdir" => fs::create_dir_all(&p).unwrap(),
        "write" => fs::write(&p, s("text")).unwrap(),
        "append" => {
            use std::io::Write;
            fs::OpenOptions::new()
                .append(true)
                .open(&p)
                .unwrap()
                .write_all(s("text").as_bytes())
                .unwrap();
        }
        "bytes" => fs::write(&p, base64_decode(&s("b64"))).unwrap(),
        "symlink" => symlink(sc.sub(&s("target")), &p).unwrap(),
        "link" => fs::hard_link(sc.at(&s("target")), &p).unwrap(),
        "rename" | "replace" => fs::rename(sc.at(&s("from")), sc.at(&s("to"))).unwrap(),
        "unlink" => fs::remove_file(&p).unwrap(),
        "chmod" => fs::set_permissions(
            &p,
            fs::Permissions::from_mode(step["mode"].as_u64().unwrap() as u32),
        )
        .unwrap(),
        "chflags" => {
            // SAFETY: a valid C string.
            assert_eq!(
                unsafe { libc::chflags(cstr(&p).as_ptr(), step["flags"].as_u64().unwrap() as _) },
                0
            );
        }
        "mkfifo" => {
            // SAFETY: a valid C string.
            assert_eq!(unsafe { libc::mkfifo(cstr(&p).as_ptr(), 0o666) }, 0);
        }
        "utime" => {
            let ns = step["ns"].as_i64().unwrap();
            let t = libc::timespec {
                tv_sec: ns / 1_000_000_000,
                tv_nsec: ns % 1_000_000_000,
            };
            // SAFETY: a valid C string and two timespecs.
            let rc = unsafe {
                libc::utimensat(
                    libc::AT_FDCWD,
                    cstr(&p).as_ptr(),
                    [t, t].as_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            assert_eq!(rc, 0);
        }
        "many" => {
            let dirs = step["dirs"].as_u64().unwrap_or(1);
            for i in 0..dirs {
                let d = match step["dir_name"].as_str() {
                    Some(n) if !n.is_empty() => pp::join(&p, &n.replace("{i}", &i.to_string())),
                    _ => p.clone(),
                };
                fs::create_dir_all(&d).unwrap();
                for j in 0..step["files"].as_u64().unwrap_or(0) {
                    let name = step["file_name"]
                        .as_str()
                        .unwrap()
                        .replace("{i}", &i.to_string())
                        .replace("{j}", &j.to_string());
                    fs::write(pp::join(&d, &name), step["text"].as_str().unwrap_or("x")).unwrap();
                }
            }
        }
        "sh" => {
            let ok = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(sc.sub(&s("command")))
                .current_dir(sc.at(&s("cwd")))
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "{}", s("command"));
        }
        other => panic!("op {other}"),
    }
}

fn base64_decode(text: &str) -> Vec<u8> {
    let table = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => 0,
        }
    };
    let bytes: Vec<u8> = text.bytes().filter(|c| *c != b'=').collect();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= table(*c) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    out
}

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A race at the moment the Python fixture patched; the returned closure undoes what is left.
fn race(sc: &Scene, spec: &Value) -> Box<dyn FnOnce()> {
    let kind = spec["kind"].as_str().unwrap();
    let swap = |dir: String, to: String, link: String| {
        fs::rename(&dir, &to).unwrap();
        symlink(&link, &dir).unwrap();
    };
    match kind {
        "access_swap" => {
            let (n, target) = (
                sc.at(spec["path"].as_str().unwrap()),
                sc.at(spec["target"].as_str().unwrap()),
            );
            let point = format!("changes.access {n}");
            hooks::set(move |p| {
                if p == point && !pp::is_symlink(&n) {
                    fs::remove_file(&n).unwrap();
                    symlink(&target, &n).unwrap();
                }
            });
            Box::new(hooks::clear)
        }
        "skip_swap" => {
            let work = sc.at(spec["path"].as_str().unwrap());
            let (to, link) = (
                sc.at(spec["rename_to"].as_str().unwrap()),
                sc.at(spec["link"].as_str().unwrap()),
            );
            let nth = spec["nth"].as_u64().unwrap();
            let point = format!("changes.skip {work}");
            let count = Cell::new(0);
            hooks::set(move |p| {
                if p == point {
                    count.set(count.get() + 1);
                    if count.get() == nth && !pp::is_symlink(&work) {
                        swap(work.clone(), to.clone(), link.clone());
                    }
                }
            });
            Box::new(hooks::clear)
        }
        "open_swap" => {
            let names: Vec<String> = spec["names"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| sc.sub(n.as_str().unwrap()))
                .collect();
            let d = sc.at(spec["dir"].as_str().unwrap());
            let (to, link) = (
                sc.at(spec["rename_to"].as_str().unwrap()),
                sc.at(spec["link"].as_str().unwrap()),
            );
            let by_path = Rc::new(RefCell::new(Vec::new()));
            let (d2, to2, seen) = (d.clone(), to.clone(), by_path.clone());
            hooks::set(move |p| {
                if let Some(rest) = p.strip_prefix("changes.open_dir ") {
                    let (how, path) = rest.split_once('|').unwrap();
                    if how == "cwd" && path.starts_with(&format!("{}/.ssh", d2)) {
                        seen.borrow_mut().push(path.to_string());
                    }
                    if names.iter().any(|n| n == path) && !pp::is_symlink(&d2) {
                        swap(d2.clone(), to2.clone(), link.clone());
                    }
                }
            });
            Box::new(move || {
                hooks::clear();
                assert!(
                    by_path.borrow().is_empty(),
                    "a new folder opened by path: {:?}",
                    by_path.borrow()
                );
                if pp::is_symlink(&d) {
                    fs::remove_file(&d).unwrap();
                    fs::rename(&to, &d).unwrap();
                }
            })
        }
        "read_swap" => {
            let target = sc.at(spec["path"].as_str().unwrap());
            let d = sc.at(spec["dir"].as_str().unwrap());
            let (to, link) = (
                sc.at(spec["rename_to"].as_str().unwrap()),
                sc.at(spec["link"].as_str().unwrap()),
            );
            let swapped = Cell::new(false);
            hooks::set(move |p| {
                if p == format!("changes.read {target}") && !pp::is_symlink(&d) {
                    swap(d.clone(), to.clone(), link.clone());
                    swapped.set(true);
                } else if p == format!("changes.read_done {target}") && swapped.get() {
                    fs::remove_file(&d).unwrap();
                    fs::rename(&to, &d).unwrap();
                    swapped.set(false);
                }
            });
            Box::new(hooks::clear)
        }
        other => panic!("race {other}"),
    }
}

fn with_race<T>(sc: &Scene, spec: &Value, f: impl FnOnce() -> T) -> T {
    if spec.is_null() {
        return f();
    }
    let undo = race(sc, spec);
    let out = f();
    undo();
    out
}

fn call(sc: &Scene, store: &Changes, step: &Value) -> Value {
    let root = &sc.root;
    let s = |k: &str| step[k].as_str().unwrap().to_string();
    match step["call"].as_str().unwrap() {
        "before" => {
            let (paths, names) = if let Some(n) = step.get("named") {
                let found = changes::named(
                    &sc.sub(n["text"].as_str().unwrap()),
                    &sc.at(n["cwd"].as_str().unwrap()),
                    &store.home,
                );
                (found.clone(), Some(found))
            } else {
                let paths = step["paths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        let p = p.as_str().unwrap();
                        if p.starts_with('/') {
                            pp::norm(&sc.sub(p))
                        } else {
                            sc.at(p)
                        }
                    })
                    .collect();
                (paths, None)
            };
            let counts = with_race(sc, &step["race"], || {
                store.before(&s("task"), &s("tree"), &paths)
            });
            let mut out = Map::new();
            out.insert("counts".into(), counts.to_json());
            out.insert(
                "record".into(),
                norm_index(store.record_text(&s("task")).as_deref(), root),
            );
            if let Some(names) = names {
                out.insert("named".into(), norm_value(&json!(names), root));
            }
            Value::Object(out)
        }
        "changes" => {
            let found = with_race(sc, &step["race"], || {
                store.changes(&s("task"), step["tree"].as_str())
            });
            let texts: Vec<String> = with_race(sc, &step["describe_race"], || {
                found.iter().map(|c| store.describe(c)).collect()
            });
            let pdf = sc.pdf.clone();
            let read = |p: &str| changes::text_of(p, Some(pdf.as_ref()));
            Value::Array(
                found
                    .iter()
                    .zip(texts)
                    .map(|(c, t)| {
                        let m = changes::describe(c, &read, None, Some(pdf.as_ref()));
                        json!({"change": norm_change(c, root), "describe": norm_text(&t, root), "module": norm_text(&m, root)})
                    })
                    .collect(),
            )
        }
        "describe_change" => {
            let d = &step["change"];
            let st = |k: &str| d[k].as_str().map(|v| sc.sub(v));
            let c = Change {
                before: st("before"),
                source: st("source"),
                note: st("note").unwrap_or_default(),
                content: d["content"].as_bool().unwrap_or(true),
                size: d["size"].as_i64(),
                mtime_ns: d["mtime_ns"].as_i64(),
                named: d["named"].as_bool().unwrap_or(false),
                before_real: st("before_real"),
                original: d["original"].as_bool().unwrap_or(false),
                ..Change::new(&st("path").unwrap(), &st("what").unwrap())
            };
            let pdf = sc.pdf.clone();
            let read = |p: &str| changes::text_of(p, Some(pdf.as_ref()));
            json!(norm_text(
                &changes::describe(&c, &read, None, Some(pdf.as_ref())),
                root
            ))
        }
        "reset" => norm_value(&json!(store.reset(&s("task"))), root),
        "forget" => norm_value(&json!(store.forget(&s("tree"))), root),
        "sweep" => {
            let keep: Vec<String> = step["keep"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            norm_value(&json!(store.sweep(&|t| keep.iter().any(|k| k == t))), root)
        }
        "made_in_tree" => json!(store.made_in_tree(&s("tree"), &sc.at(&s("path")))),
        "index" => {
            let f = pp::join(&store.folder(&s("tree"), &s("task")), changes::INDEX);
            norm_index(fs::read_to_string(f).ok().as_deref(), root)
        }
        "index_mtime" => {
            use std::os::unix::fs::MetadataExt;
            let md = fs::metadata(pp::join(
                &store.folder(&s("tree"), &s("task")),
                changes::INDEX,
            ))
            .unwrap();
            json!(md.mtime() * 1_000_000_000 + md.mtime_nsec())
        }
        "set_index_mtime" => {
            let f = pp::join(&store.folder(&s("tree"), &s("task")), changes::INDEX);
            fs_op(
                sc,
                &json!({"op": "utime", "path": pp::relative_to(&f, root), "ns": step["ns"]}),
            );
            Value::Null
        }
        "snapshot" => snapshot(&sc.at(&s("dir")), root),
        "restart" => Value::Null,
        "probe_read" => match changes::read_file(&sc.at(&s("path")), None, None) {
            None => Value::Null,
            Some(b) => json!(base64_encode(&b)),
        },
        "probe_same" => json!(changes::same(&sc.at(&s("a")), &sc.at(&s("b")))),
        "text_of" => match changes::text_of(&sc.at(&s("path")), Some(sc.pdf.as_ref())) {
            None => Value::Null,
            Some((t, notes)) => json!([
                norm_text(&t, root),
                notes
                    .iter()
                    .map(|(p, t, x)| json!([p, t, x]))
                    .collect::<Vec<_>>()
            ]),
        },
        other => panic!("call {other}"),
    }
}

fn cleanup(top: &str) {
    for e in walk_all(top) {
        let c = cstr(&e);
        if !pp::is_symlink(&e) {
            // SAFETY: a valid C string.
            unsafe { libc::chflags(c.as_ptr(), 0) };
            let mode = if pp::is_dir(&e) { 0o755 } else { 0o644 };
            let _ = fs::set_permissions(&e, fs::Permissions::from_mode(mode));
        }
    }
}

fn walk_all(top: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(top) {
        for e in rd.flatten() {
            let p = pp::join(top, &e.file_name().to_string_lossy());
            out.push(p.clone());
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                out.extend(walk_all(&p));
            }
        }
    }
    out
}

/// Runs one scenario; returns a description of every output that differs.
fn replay(scenario: &Value) -> Vec<String> {
    // the same length of name as Python's mkdtemp (a link's size is its target's length)
    let tmp = tempfile::Builder::new()
        .prefix("tmp")
        .rand_bytes(8)
        .tempdir()
        .unwrap();
    let root = fs::canonicalize(tmp.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let home = pp::join(&root, scenario["home"].as_str().unwrap_or("home"));
    fs::create_dir_all(&home).unwrap();
    let sc = Scene {
        root: root.clone(),
        home,
        cfg: scenario.get("store").cloned().unwrap_or(json!({})),
        pdf: recorded_pdf(),
    };
    let mut store = sc.store();
    let mut diffs = Vec::new();
    let expected = scenario["outputs"].as_array().unwrap();
    let mut n = 0;
    for step in scenario["steps"].as_array().unwrap() {
        if step.get("op").is_some() {
            fs_op(&sc, step);
            continue;
        }
        let got = call(&sc, &store, step);
        if got != expected[n] {
            diffs.push(format!(
                "{} step {} ({}):\n  python: {}\n  rust:   {}",
                scenario["name"], n, step["call"], expected[n], got
            ));
        }
        n += 1;
        if step["call"] == "restart" {
            store = sc.store();
        }
    }
    cleanup(&root);
    diffs
}

fn run_named(filter: impl Fn(&str) -> bool) -> (usize, Vec<String>) {
    let mut diffs = Vec::new();
    let mut count = 0;
    for sc in fixture()["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        if filter(name) {
            count += 1;
            diffs.extend(replay(sc));
        }
    }
    (count, diffs)
}

fn check(filter: impl Fn(&str) -> bool) {
    let (count, diffs) = run_named(filter);
    assert!(count > 0);
    assert!(
        diffs.is_empty(),
        "{} of the outputs differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

// decision: GATE-539 (made_in_tree), L39 (the change record)
#[test]
fn scenarios_from_test_changes() {
    check(|n| {
        [
            "live_perl_replace",
            "corrected_copy_beside_original",
            "pdf_covered_by_boxes",
            "new_pdf_named",
            "same_bytes_no_change",
            "plain_edit_and_new_file",
            "closed_store_and_trash",
            "restart_and_forget",
            "loop_round_reset",
        ]
        .contains(&n)
            || n.starts_with("describe_no_copy_")
    });
}

#[test]
fn history_files_by_identity() {
    check(|n| n.starts_with("history_"));
}

#[test]
fn walks_and_limits() {
    check(|n| {
        [
            "walk_only_through_checked_handle",
            "fact_limits_across_new_folders",
            "flat_new_files_share_a_page",
            "named_file_past_walk_limit",
            "empty_new_folders_count",
            "deep_new_folders",
            "stop_fact_every_listed_folder",
            "npm_install_new_folder",
            "listed_folder_removed_and_replaced",
        ]
        .contains(&n)
    });
}

#[test]
fn renames_backups_and_pairing() {
    check(|n| {
        n.starts_with("edit_keeps_backup_")
            || [
                "new_named_docx_whole",
                "renamed_named_file_diffed",
                "pure_rename",
                "corrected_copy_other_name",
                "named_file_unreadable",
                "fifo_never_blocks",
                "moved_into_new_folder",
                "case_only_rename",
                "in_place_no_backup",
                "backup_changed_too",
                "pairing_in_bulk",
                "text_preview_cut",
                "edit_crlf_and_binary_diff",
            ]
            .contains(&n)
    });
}

#[test]
fn protection_and_races() {
    check(|n| {
        n.starts_with("named_swapped_for_link_")
            || [
                "sibling_read_through_checked_fd",
                "siblings_listed_through_fd",
                "new_links_never_read",
                "command_naming_closed_store",
                "cd_home_history_fact",
                "declared_file_under_new_folder",
                "files_copied_into_made_folder",
                "pairing_reads_few",
                "resuming_tree_survives_sweep",
                "locked_clone_removable",
                "index_written_on_change_only",
                "access_swap_never_cloned",
                "folder_swapped_between_check_and_listing",
                "named_file_replaced_by_link",
            ]
            .contains(&n)
    });
}

#[test]
fn every_scenario_is_replayed() {
    let names: Vec<&str> = fixture()["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    let covered = |n: &str| {
        n.starts_with("describe_no_copy_")
            || n.starts_with("history_")
            || n.starts_with("edit_keeps_backup_")
            || n.starts_with("named_swapped_for_link_")
            || [
                "live_perl_replace",
                "corrected_copy_beside_original",
                "pdf_covered_by_boxes",
                "new_pdf_named",
                "same_bytes_no_change",
                "plain_edit_and_new_file",
                "closed_store_and_trash",
                "restart_and_forget",
                "loop_round_reset",
                "walk_only_through_checked_handle",
                "fact_limits_across_new_folders",
                "flat_new_files_share_a_page",
                "named_file_past_walk_limit",
                "empty_new_folders_count",
                "deep_new_folders",
                "stop_fact_every_listed_folder",
                "npm_install_new_folder",
                "listed_folder_removed_and_replaced",
                "new_named_docx_whole",
                "renamed_named_file_diffed",
                "pure_rename",
                "corrected_copy_other_name",
                "named_file_unreadable",
                "fifo_never_blocks",
                "moved_into_new_folder",
                "case_only_rename",
                "in_place_no_backup",
                "backup_changed_too",
                "pairing_in_bulk",
                "text_preview_cut",
                "edit_crlf_and_binary_diff",
                "sibling_read_through_checked_fd",
                "siblings_listed_through_fd",
                "new_links_never_read",
                "command_naming_closed_store",
                "cd_home_history_fact",
                "declared_file_under_new_folder",
                "files_copied_into_made_folder",
                "pairing_reads_few",
                "resuming_tree_survives_sweep",
                "locked_clone_removable",
                "index_written_on_change_only",
                "access_swap_never_cloned",
                "folder_swapped_between_check_and_listing",
                "named_file_replaced_by_link",
            ]
            .contains(&n)
    };
    let missing: Vec<&&str> = names.iter().filter(|n| !covered(n)).collect();
    assert!(missing.is_empty(), "scenarios no test replays: {missing:?}");
}

/// `named()` on command texts. Deliberate difference: Python iterates `{word, embedded path}` as a hash-seeded set,
/// so for a word holding an embedded path its order changes from run to run (the fixture keeps those cases sorted);
/// Rust always takes the word first. Those cases are compared as sorted lists.
#[test]
fn named_paths_match_python() {
    let mut diffs = Vec::new();
    for case in fixture()["named"].as_array().unwrap() {
        let mut got = changes::named(
            case["text"].as_str().unwrap(),
            case["cwd"].as_str().unwrap(),
            case["home"].as_str().unwrap(),
        );
        if !case["ordered"].as_bool().unwrap() {
            got.sort();
        }
        if json!(got) != case["found"] {
            diffs.push(format!(
                "{}\n  python: {}\n  rust:   {}",
                case["text"],
                case["found"],
                json!(got)
            ));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}

/// `repr(float)`, as the fixture keeps ratios.
fn py(x: f64) -> Value {
    json!(ferb_py::json::python_float(x))
}

#[test]
fn difflib_matches_python() {
    let mut n = 0;
    for case in fixture()["difflib"].as_array().unwrap() {
        if let Some(a) = case.get("a") {
            let strs = |v: &Value| -> Vec<String> {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_str().unwrap().to_string())
                    .collect()
            };
            let (a, b) = (strs(a), strs(&case["b"]));
            let mut j = SequenceMatcher::new(&a, &b, true);
            let blocks: Vec<Value> = j
                .get_matching_blocks()
                .iter()
                .map(|m| json!([m.0, m.1, m.2]))
                .collect();
            assert_eq!(json!(blocks), case["blocks"], "{a:?} {b:?}");
            assert_eq!(py(j.ratio()), case["ratio"]);
            let mut m = SequenceMatcher::new(&a, &b, false);
            assert_eq!(py(m.ratio()), case["ratio_nojunk"]);
            assert_eq!(py(m.quick_ratio()), case["quick"]);
            assert_eq!(py(m.real_quick_ratio()), case["real_quick"]);
            assert_eq!(
                json!(unified_diff(&a, &b, "before", "now", 2)),
                case["diff2"]
            );
            assert_eq!(
                json!(unified_diff(&a, &b, "before", "now", 3)),
                case["diff3"]
            );
        } else if let Some(sa) = case.get("sa") {
            let r = str_ratio(sa.as_str().unwrap(), case["sb"].as_str().unwrap());
            assert_eq!(py(r), case["ratio"], "{sa} {}", case["sb"]);
        } else {
            assert_eq!(
                json!(splitlines(case["text"].as_str().unwrap())),
                case["lines"]
            );
        }
        n += 1;
    }
    assert!(n > 200);
}

#[test]
fn nothing_blocks_on_a_fifo() {
    let tmp = tempfile::TempDir::new().unwrap();
    let fifo = tmp.path().join("n.txt");
    let c = cstr(&fifo.to_string_lossy());
    // SAFETY: a valid C string.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o666) }, 0);
    let f = fifo.to_string_lossy().into_owned();
    let t = std::thread::spawn(move || {
        (
            changes::read_file(&f, None, None),
            changes::same(&f, "/nonexistent"),
        )
    });
    let start = std::time::Instant::now();
    while !t.is_finished() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "blocked on a FIFO"
        );
        std::thread::yield_now();
    }
    assert_eq!(t.join().unwrap(), (None, false));
}
