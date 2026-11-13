//! The file tools and S1 recovery against what the Python code did (decision: ENG-3).
//! `rust/fixtures/fs/make.py` ran the real ferb/act/fs tools and recovery.py on invented trees and wrote `tools.json`:
//! per case the tree as built, then each step (a tool call, a recovery call, a file operation of the owner's or a
//! racer's) with its result, the tree after it and the recovery table's rows. Each test here rebuilds the tree in a
//! temporary folder, replays the steps through the Rust port (racers at the same named points: crate::hooks) and must
//! get the same results, trees and rows after the same normalization. Temporary folders only; the Trash is a folder in
//! the case, injected where the Python tests inject it.
//! decisions: GATE-539, HAND-432, HAND-206, HAND-216, HAND-408, HAND-517, GATE-203

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::{Arc, Mutex, Once};

use chrono::{DateTime, FixedOffset, TimeZone};
use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{Dict, Identity, PathCheck, ToolContext, ToolResult};
use ferb_fs::files::{self, FsEnv, Out, Raised, TrashLocator};
use ferb_fs::look::{self, LookError, Renderer};
use ferb_fs::recovery::{self, RestoreError, Snapshots};
use ferb_fs::{hooks, sys, usage};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fs/tools.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::env::set_var("TZ", "UTC");
        // SAFETY: umask has no failure mode.
        unsafe { libc::umask(0o022) };
    });
}

// ---------- the tree ----------

fn data_of(spec: &Value) -> Vec<u8> {
    match spec {
        Value::String(s) => s.as_bytes().to_vec(),
        Value::Object(o) => hex::decode(o["hex"].as_str().unwrap()).unwrap(),
        other => panic!("{other}"),
    }
}

fn subst(text: &str, root: &str) -> String {
    text.replace("$ROOT", root)
}

fn set_mtime(path: &str, ns: i128) {
    let c = CString::new(path).unwrap();
    let ts = libc::timespec {
        tv_sec: (ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (ns % 1_000_000_000) as libc::c_long,
    };
    let times = [ts, ts];
    // SAFETY: valid C string and two timespecs.
    unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
}

fn chflags(path: &str, flags: u32) {
    let c = CString::new(path).unwrap();
    // SAFETY: valid C string.
    unsafe { libc::lchflags(c.as_ptr(), flags) };
}

fn walk_paths(dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            walk_paths(&p, out);
        }
        out.push(p.to_string_lossy().into_owned());
    }
}

fn build(root: &str, tree: &[Value], old_ns: i128) {
    fs::create_dir_all(root).unwrap();
    for item in tree {
        let kind = item[0].as_str().unwrap();
        let p = format!("{root}/{}", item[1].as_str().unwrap());
        let parent = Path::new(&p).parent().unwrap().to_path_buf();
        match kind {
            "dir" => {
                fs::create_dir_all(&p).unwrap();
                fs::set_permissions(
                    &p,
                    fs::Permissions::from_mode(item[2].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
            "file" => {
                fs::create_dir_all(&parent).unwrap();
                fs::write(&p, data_of(&item[2])).unwrap();
                fs::set_permissions(
                    &p,
                    fs::Permissions::from_mode(item[3].as_u64().unwrap() as u32),
                )
                .unwrap();
                for (name, value) in item
                    .get(4)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default()
                {
                    let (c, n) = (
                        CString::new(p.as_str()).unwrap(),
                        CString::new(name).unwrap(),
                    );
                    let v = value.as_str().unwrap().as_bytes();
                    // SAFETY: valid C strings and value buffer.
                    unsafe {
                        libc::setxattr(c.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0)
                    };
                }
            }
            "link" => {
                fs::create_dir_all(&parent).unwrap();
                std::os::unix::fs::symlink(subst(item[2].as_str().unwrap(), root), &p).unwrap();
            }
            "hardlink" => {
                fs::hard_link(format!("{root}/{}", item[2].as_str().unwrap()), &p).unwrap()
            }
            "fifo" => {
                let c = CString::new(p.as_str()).unwrap();
                // SAFETY: valid C string.
                unsafe { libc::mkfifo(c.as_ptr(), 0o644) };
            }
            "flags" => {}
            other => panic!("{other}"),
        }
    }
    let mut all = Vec::new();
    walk_paths(Path::new(root), &mut all);
    for p in &all {
        set_mtime(p, old_ns);
    }
    set_mtime(root, old_ns);
    for item in tree {
        if item[0] == "flags" {
            chflags(
                &format!("{root}/{}", item[1].as_str().unwrap()),
                item[2].as_u64().unwrap() as u32,
            );
        }
    }
}

fn snapshot(root: &str) -> Vec<Value> {
    fn walk(root: &str, dir: &str, out: &mut Vec<Value>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut dirs = Vec::new();
        for n in &names {
            let p = format!("{dir}/{n}");
            let st = fs::symlink_metadata(&p).unwrap();
            let rel = p[root.len() + 1..].to_string();
            let mut e = Dict::new();
            e.insert("path".into(), json!(rel));
            e.insert("mode".into(), json!(st.mode() & 0o7777));
            e.insert(
                "flags".into(),
                json!(sys::lstat(&p).unwrap().flags & 0xFFFF),
            );
            let ft = st.file_type();
            if ft.is_symlink() {
                e.insert("kind".into(), json!("link"));
                e.insert(
                    "target".into(),
                    json!(fs::read_link(&p).unwrap().to_string_lossy()),
                );
                if fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
                    // os.walk lists it with the folders but never walks in
                }
            } else if ft.is_dir() {
                e.insert("kind".into(), json!("dir"));
                dirs.push(p.clone());
            } else if ft.is_file() {
                e.insert("kind".into(), json!("file"));
                e.insert("size".into(), json!(st.size()));
                e.insert("nlink".into(), json!(st.nlink()));
                e.insert(
                    "mtime_ns".into(),
                    json!(st.mtime() * 1_000_000_000 + st.mtime_nsec()),
                );
                let readable = sys::access(&p, libc::R_OK, true);
                e.insert(
                    "sha".into(),
                    if readable {
                        json!(hex::encode(Sha256::digest(fs::read(&p).unwrap())))
                    } else {
                        Value::Null
                    },
                );
                if readable {
                    let fd =
                        sys::open(sys::At::Cwd, &p, sys::O_RDONLY | sys::O_NOFOLLOW, 0).unwrap();
                    let mut xs: Vec<(String, String)> = sys::xattrs(fd.raw())
                        .into_iter()
                        .filter(|(k, _)| !k.starts_with(b"com.apple."))
                        .map(|(k, v)| (String::from_utf8(k).unwrap(), hex::encode(v)))
                        .collect();
                    xs.sort();
                    e.insert(
                        "xattrs".into(),
                        Value::Object(xs.into_iter().map(|(k, v)| (k, json!(v))).collect()),
                    );
                }
            } else {
                e.insert("kind".into(), json!("other"));
            }
            out.push(Value::Object(e));
        }
        for d in dirs {
            walk(root, &d, out);
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

// ---------- normalization (make.py `normalize`) ----------

const IDS: [&str; 6] = [
    "device",
    "inode",
    "owner_uid",
    "owner_gid",
    "saved_dev",
    "saved_ino",
];

fn norm_text(s: &str, root: &str) -> String {
    let s = s.replace(root, "$ROOT");
    let rx = fancy_regex::Regex::new(
        r#"(?<= )[0-9a-f]{8}(?=[."/ ]|$)|(?<=\.)[0-9a-f]{8}(?=\.ferb-tmp)"#,
    )
    .unwrap();
    rx.replace_all(&s, "<hex>").into_owned()
}

fn normalize(v: &Value, root: &str, key: &str, new_ns: i64) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (norm_text(k, root), normalize(x, root, k, new_ns)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|x| normalize(x, root, "", new_ns)).collect()),
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            if IDS.contains(&key) {
                json!("<id>")
            } else if key == "mtime_ns" && n.as_i64().is_some_and(|x| x > new_ns) {
                json!("<new>")
            } else {
                v.clone()
            }
        }
        Value::String(s) => {
            if key == "modified_at" && s.get(..4).is_some_and(|y| y >= "2020") {
                json!("<new>")
            } else {
                json!(norm_text(s, root))
            }
        }
        _ => v.clone(),
    }
}

// ---------- the fakes ----------

struct Trash {
    path: String,
}

impl TrashLocator for Trash {
    fn trash_url(&self, _near: &str, create: bool) -> Option<String> {
        if create && !Path::new(&self.path).exists() {
            fs::create_dir(&self.path).unwrap();
        }
        Some(self.path.clone())
    }
}

struct FakeClock(Mutex<DateTime<FixedOffset>>);

impl Clock for FakeClock {
    fn now(&self) -> DateTime<FixedOffset> {
        *self.0.lock().unwrap()
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
    fn sleep_until(&self, _: DateTime<FixedOffset>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct Seq(Mutex<u32>);

impl IdSource for Seq {
    fn new_id(&self) -> String {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        format!("rec-{:06}", *n)
    }
}

/// make.py `fake_picture` / `fake_pdf`.
struct FakeRenderer;

impl Renderer for FakeRenderer {
    fn picture(&self, data: &[u8], max_edge: u32) -> Result<(Vec<u8>, Dict), LookError> {
        if data.starts_with(b"BAD") {
            return Err(LookError::Undecodable("not a picture".into()));
        }
        let digest = &hex::encode(Sha256::digest(data))[..16];
        let mut d = Dict::new();
        d.insert("format".into(), json!("public.png"));
        d.insert("pixels".into(), look::pair(data.len() as i64, 3));
        d.insert("width".into(), json!(max_edge.min(40)));
        d.insert("height".into(), json!(3));
        Ok(([b"JPEG".as_slice(), digest.as_bytes()].concat(), d))
    }

    fn pdf_page(
        &self,
        data: &[u8],
        number: i64,
        max_edge: u32,
    ) -> Result<(Vec<u8>, Dict), LookError> {
        let pages = data.windows(6).filter(|w| *w == b"/Page ").count() as i64;
        if data.windows(6).any(|w| w == b"LOCKED") {
            return Err(LookError::Undecodable(
                "the PDF is locked with a password".into(),
            ));
        }
        if !(1..=pages).contains(&number) {
            return Err(LookError::NoPage(pages));
        }
        let mut d = Dict::new();
        d.insert("format".into(), json!("com.adobe.pdf"));
        d.insert("page".into(), json!(number));
        d.insert("pages".into(), json!(pages));
        d.insert("width".into(), json!(max_edge));
        d.insert("height".into(), json!(900));
        d.insert("text".into(), json!(format!("page {number} text")));
        Ok((format!("JPEG-PAGE{number}").into_bytes(), d))
    }
}

fn redact_fake(v: &Value) -> Value {
    match v {
        Value::String(s) => json!(s.replace("4111 1111 1111 1111", "[card]")),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, x)| (k.clone(), redact_fake(x))).collect())
        }
        other => other.clone(),
    }
}

// ---------- racers and the owner's file operations ----------

fn run_ops(ops: &[Value], root: &str) {
    for op in ops {
        let kind = op[0].as_str().unwrap();
        let p = format!("{root}/{}", op[1].as_str().unwrap());
        match kind {
            "write" => fs::write(&p, data_of(&op[2])).unwrap(),
            "unlink" => fs::remove_file(&p).unwrap(),
            "symlink" => {
                std::os::unix::fs::symlink(subst(op[2].as_str().unwrap(), root), &p).unwrap()
            }
            "rename" => fs::rename(&p, format!("{root}/{}", op[2].as_str().unwrap())).unwrap(),
            "mkdir" => fs::create_dir(&p).unwrap(),
            "rmtree" => fs::remove_dir_all(&p).unwrap(),
            "hardlink" => fs::hard_link(format!("{root}/{}", op[2].as_str().unwrap()), &p).unwrap(),
            "chflags" => chflags(&p, op[2].as_u64().unwrap() as u32),
            other => panic!("{other}"),
        }
    }
}

fn set_hooks(spec: Option<&Value>, root: &str) {
    let Some(spec) = spec.and_then(Value::as_object).cloned() else {
        hooks::clear();
        return;
    };
    let root = root.to_string();
    let mut done = HashSet::new();
    hooks::set(move |point| {
        if let Some(ops) = spec.get(point) {
            if done.insert(point.to_string()) {
                run_ops(ops.as_array().unwrap(), &root);
            }
        }
    });
}

// ---------- the case ----------

struct Case {
    root: String,
    home: String,
    ctx: ToolContext,
    env: FsEnv,
    db: Arc<Mutex<Connection>>,
    clock: Arc<FakeClock>,
    ids: Arc<Seq>,
    snapshots: Option<Snapshots>,
    rt: tokio::runtime::Runtime,
    rules: Arc<Vec<(String, Vec<String>, String)>>,
    _keep: tempfile::TempDir,
}

fn result_json(r: &ToolResult) -> Value {
    let mut out = json!({
        "content": Value::Object(r.content.clone()),
        "is_error": r.is_error,
        "read_sources": r.read_sources.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
    });
    if let Some(img) = &r.image {
        out["image"] = json!(String::from_utf8(img.clone()).unwrap());
    }
    out
}

fn out_json(o: Out) -> Value {
    match o {
        Ok(r) => result_json(&r),
        Err(Raised::Error(t)) => json!({"raised": "RuntimeError", "text": t}),
        Err(Raised::Refused(t)) => json!({"raised": "Refused", "text": t}),
        Err(Raised::Os(e)) => json!({"raised": usage::error_class(&e), "text": e.to_string()}),
    }
}

fn restore_json(r: Result<String, RestoreError>) -> Value {
    match r {
        Ok(p) => json!(p),
        Err(RestoreError::NotRestorable(t)) => json!({"raised": "NotRestorable", "text": t}),
        Err(RestoreError::Os(e)) => {
            json!({"raised": usage::error_class(&e), "text": e.to_string()})
        }
        Err(RestoreError::Other(t)) => json!({"raised": "Other", "text": t}),
    }
}

impl Case {
    fn new(case: &Value) -> Case {
        let keep = tempfile::tempdir().unwrap();
        let base = ferb_fs::pypath::realpath(keep.path().to_str().unwrap());
        let name = case["name"].as_str().unwrap();
        let root = format!("{base}/{name}");
        let home = format!("{root}/home");
        let rules: Vec<(String, Vec<String>, String)> = case
            .get("closed")
            .and_then(Value::as_array)
            .map(|rs| {
                rs.iter()
                    .map(|r| {
                        (
                            subst(r["under"].as_str().unwrap(), &root),
                            r["access"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|a| a.as_str().unwrap().to_string())
                                .collect(),
                            r["say"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rules = Arc::new(rules);
        let closed = {
            let rules = rules.clone();
            move |path: &Path, access: &str| -> Option<String> {
                let p = path.to_string_lossy();
                rules
                    .iter()
                    .find(|(under, accesses, _)| {
                        accesses.iter().any(|a| a == access)
                            && (p == *under || p.starts_with(&format!("{under}/")))
                    })
                    .map(|(_, _, say)| say.clone())
            }
        };
        let ctx = ToolContext {
            task_id: "t1".into(),
            worker_id: "w1".into(),
            request_id: "r1".into(),
            identity: Identity::Owner,
            scopes: ["fs:read", "fs:write"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            device_id: "dev".into(),
            span_id: None,
            workspace: format!("{home}/.ferb/workspace"),
            owner_roots: vec![],
            untrusted_roots: vec![format!("{root}/Downloads")],
            intent_id: None,
            observe_only: false,
            probe: false,
            path_closed: (!rules.is_empty()).then(|| PathCheck(Arc::new(closed))),
            escape: None,
            terminal: None,
            folders: vec![],
        };
        let trash = format!(
            "{root}/{}",
            case.get("trash")
                .and_then(Value::as_str)
                .unwrap_or("home/.Trash")
        );
        let env = FsEnv {
            home: home.clone(),
            trash: Arc::new(Trash { path: trash }),
            full_disk_access: Arc::new(|| false),
        };
        let db_path = format!("{base}/{name}.db");
        ferb_store::Db::open(&db_path).unwrap().migrate().unwrap();
        let db = Arc::new(Mutex::new(Connection::open(&db_path).unwrap()));
        let base_time = DateTime::parse_from_rfc3339("2026-03-04T05:06:07+00:00").unwrap();
        Case {
            root,
            home,
            ctx,
            env,
            db,
            clock: Arc::new(FakeClock(Mutex::new(base_time))),
            ids: Arc::new(Seq(Mutex::new(0))),
            snapshots: None,
            rt: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
            rules,
            _keep: keep,
        }
    }

    fn arg(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => json!(subst(s, &self.root)),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), self.arg(x))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.arg(x)).collect()),
            other => other.clone(),
        }
    }

    fn rows(&self) -> Value {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM recovery ORDER BY rowid")
            .unwrap();
        let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map([], |r| {
                let mut d = Dict::new();
                for (i, n) in names.iter().enumerate() {
                    let v: rusqlite::types::Value = r.get(i)?;
                    d.insert(
                        n.clone(),
                        match v {
                            rusqlite::types::Value::Null => Value::Null,
                            rusqlite::types::Value::Integer(n) => json!(n),
                            rusqlite::types::Value::Text(t) => json!(t),
                            other => panic!("{other:?}"),
                        },
                    );
                }
                Ok(Value::Object(d))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        Value::Array(rows)
    }

    fn closed_fn(&self) -> Option<impl Fn(&str, &str) -> Option<String> + '_> {
        (!self.rules.is_empty()).then_some(move |p: &str, a: &str| {
            (self.ctx.path_closed.as_ref().unwrap().0)(Path::new(p), a)
        })
    }

    fn tool(&self, name: &str, args: &Value) -> Value {
        let (ctx, env) = (&self.ctx, &self.env);
        let a = args.clone();
        let o = match name {
            "fs.list" => files::fs_list(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.read" => files::fs_read(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.write" => files::fs_write(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.edit" => files::fs_edit(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.copy" => files::fs_copy(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.move" => files::fs_move(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.trash" => files::fs_trash(&serde_json::from_value(a).unwrap(), ctx, env),
            "fs.look" => {
                look::fs_look(&serde_json::from_value(a).unwrap(), ctx, env, &FakeRenderer)
            }
            "fs.disk_usage" => usage::fs_disk_usage(&serde_json::from_value(a).unwrap(), ctx, env),
            other => panic!("{other}"),
        };
        out_json(o)
    }

    fn run(&mut self, s: &Value) -> Value {
        if let Some(ops) = s.get("do") {
            run_ops(ops.as_array().unwrap(), &self.root);
            return Value::Null;
        }
        if let Some(secs) = s.get("advance") {
            let mut at = self.clock.0.lock().unwrap();
            *at += chrono::Duration::seconds(secs.as_i64().unwrap());
            return Value::Null;
        }
        let args = self.arg(s.get("args").unwrap_or(&json!({})));
        if let Some(tool) = s.get("tool") {
            return self.tool(tool.as_str().unwrap(), &args);
        }
        let root = format!("{}/.ferb/recovery", self.home);
        let st = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
        let fail = |f: files::Fail| match f {
            files::Fail::Os(e) => json!({"raised": usage::error_class(&e), "text": e.to_string()}),
            other => json!({"raised": "Other", "text": format!("{other:?}")}),
        };
        match s["fn"].as_str().unwrap() {
            "preserve" => {
                let conn = self.db.lock().unwrap();
                match recovery::preserve(
                    &st("path").unwrap(),
                    &st("reason").unwrap(),
                    st("task_id").as_deref(),
                    &conn,
                    self.clock.as_ref(),
                    self.ids.as_ref(),
                    &root,
                ) {
                    Ok(id) => json!(id),
                    Err(f) => fail(f),
                }
            }
            "mark_written" => {
                let conn = self.db.lock().unwrap();
                recovery::mark_written(&st("id").unwrap(), &st("path").unwrap(), &conn).unwrap();
                Value::Null
            }
            "discard_if_unchanged" => {
                let conn = self.db.lock().unwrap();
                json!(recovery::discard_if_unchanged(
                    &st("id").unwrap(),
                    &st("path").unwrap(),
                    &conn,
                    &root
                )
                .unwrap())
            }
            "record_trash" => {
                let conn = self.db.lock().unwrap();
                json!(recovery::record_trash(
                    &st("original").unwrap(),
                    st("trashed_to").as_deref(),
                    args.get("device").and_then(Value::as_i64),
                    args.get("inode").and_then(Value::as_u64),
                    &st("reason").unwrap_or_else(|| "trash".into()),
                    st("task_id").as_deref(),
                    &conn,
                    self.clock.as_ref(),
                    self.ids.as_ref(),
                )
                .unwrap())
            }
            "trash_then_record" => {
                let r = files::fs_trash(
                    &serde_json::from_value(args.clone()).unwrap(),
                    &self.ctx,
                    &self.env,
                )
                .unwrap();
                let c = &r.content;
                if let Some(to) = c
                    .get("trashed_to")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    let conn = self.db.lock().unwrap();
                    recovery::record_trash(
                        c["path"].as_str().unwrap(),
                        Some(to),
                        c.get("device").and_then(Value::as_i64),
                        c.get("inode").and_then(Value::as_u64),
                        if r.is_error {
                            recovery::TRASH_UNVERIFIED
                        } else {
                            "trash"
                        },
                        Some("t1"),
                        &conn,
                        self.clock.as_ref(),
                        self.ids.as_ref(),
                    )
                    .unwrap();
                }
                result_json(&r)
            }
            "sql" => {
                let conn = self.db.lock().unwrap();
                let params: Vec<String> = args
                    .get("params")
                    .and_then(Value::as_array)
                    .map(|p| p.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                    .unwrap_or_default();
                conn.execute(
                    &st("sql").unwrap(),
                    rusqlite::params_from_iter(params.iter()),
                )
                .unwrap();
                Value::Null
            }
            "restore" => {
                let conn = self.db.lock().unwrap();
                let closed = self.closed_fn();
                let c = closed
                    .as_ref()
                    .map(|c| c as &dyn Fn(&str, &str) -> Option<String>);
                restore_json(recovery::restore(
                    &st("id").unwrap(),
                    &conn,
                    &root,
                    c,
                    &self.home,
                ))
            }
            "recent" => {
                let conn = self.db.lock().unwrap();
                let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100);
                let (rows, total) = recovery::recent(&conn, &root, limit).unwrap();
                json!([rows, total])
            }
            "find" => {
                let conn = self.db.lock().unwrap();
                let r = recovery::find(&conn, &st("ref").unwrap()).unwrap();
                json!({"id": r.id, "task_id": r.task_id, "kind": r.kind, "original": r.original,
                       "saved_to": r.saved_to, "reason": r.reason, "at": r.at, "written_sha256": r.written_sha256,
                       "saved_dev": r.saved_dev, "saved_ino": r.saved_ino})
            }
            "purge_before" => {
                let conn = self.db.lock().unwrap();
                json!(recovery::purge_before(
                    &st("cutoff").unwrap(),
                    &conn,
                    &root,
                    st("backstop").as_deref()
                )
                .unwrap())
            }
            "copy_into" => {
                let src = sys::open(sys::At::Cwd, &st("src").unwrap(), sys::O_RDONLY, 0).unwrap();
                let dir = sys::open(
                    sys::At::Cwd,
                    &st("dir").unwrap(),
                    sys::O_RDONLY | sys::O_DIRECTORY,
                    0,
                )
                .unwrap();
                match recovery::copy_into(src.raw(), dir.raw(), &st("name").unwrap()) {
                    Ok(sha) => json!(sha),
                    Err(e) => json!({"raised": usage::error_class(&e), "text": e.to_string()}),
                }
            }
            "short" => json!(recovery::short(&st("path").unwrap(), &self.home)),
            "snapshots" => {
                let names: Vec<String> = args["takes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_str().unwrap().to_string())
                    .collect();
                let on = !names.is_empty();
                let queue = Arc::new(Mutex::new(names));
                let take: recovery::Take = Arc::new(move || {
                    let n = queue.lock().unwrap().remove(0);
                    if n == "RAISE" {
                        Err("TimeoutError".into())
                    } else {
                        Ok(Some(n))
                    }
                });
                self.snapshots = Some(Snapshots::new(
                    on.then_some(take),
                    self.db.clone(),
                    self.clock.clone(),
                    self.ids.clone(),
                    Arc::new(redact_fake),
                ));
                Value::Null
            }
            "request" => {
                let snaps = self.snapshots.as_ref().unwrap();
                let task = st("task_id");
                let changes = args
                    .get("changes")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let reason = st("reason").unwrap();
                Value::Object(
                    self.rt
                        .block_on(snaps.request(task.as_deref(), &reason, changes)),
                )
            }
            "snapshot_tool" => {
                let snaps = self.snapshots.as_ref().unwrap();
                let a: recovery::SnapshotArgs = serde_json::from_value(args.clone()).unwrap();
                result_json(
                    &self
                        .rt
                        .block_on(recovery::recovery_snapshot(&a, "t1", snaps)),
                )
            }
            "last_failure" => json!(self.snapshots.as_ref().unwrap().last_failure()),
            "purge_snapshots" => {
                let answers: HashMap<String, Value> = args
                    .get("delete")
                    .and_then(Value::as_object)
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                let answers = Arc::new(answers);
                let delete: recovery::Delete = Arc::new(move |name: String| {
                    let a = answers.get(&name).cloned().unwrap_or(json!(true));
                    Box::pin(async move { Ok(a.as_bool().unwrap_or(false)) })
                });
                let deleting = args
                    .get("deleting")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let mut gone = Vec::new();
                let n = self
                    .rt
                    .block_on(recovery::purge_snapshots(
                        &st("cutoff").unwrap(),
                        &self.db,
                        deleting.then_some(delete),
                        |name, d| gone.push(json!([name, d])),
                    ))
                    .unwrap();
                json!({"done": n, "gone": gone})
            }
            other => panic!("{other}"),
        }
    }
}

fn clear_flags(root: &str) {
    let mut all = Vec::new();
    walk_paths(Path::new(root), &mut all);
    for p in all {
        chflags(&p, 0);
        if fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
        }
    }
}

/// One case: every step's result, tree and rows as Python's. Returns the differences.
fn play(case: &Value, old_ns: i128, new_ns: i64) -> Vec<String> {
    let mut c = Case::new(case);
    build(
        &c.root,
        case["tree"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        old_ns,
    );
    let mut diffs = Vec::new();
    let root = c.root.clone();
    let mut last = normalize(&Value::Array(snapshot(&root)), &root, "", new_ns);
    if last != case["tree_before"] {
        diffs.push(format!(
            "{}: the built tree differs:\n{last}\n{}",
            case["name"], case["tree_before"]
        ));
    }
    for (i, s) in case["steps"].as_array().unwrap().iter().enumerate() {
        set_hooks(s.get("hooks"), &root);
        let off: Vec<&str> = s
            .get("off")
            .and_then(Value::as_array)
            .map(|o| o.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        hooks::set_off(&off);
        let ret = c.run(s);
        hooks::clear();
        hooks::clear_off();
        let ret = normalize(&ret, &root, "", new_ns);
        let tree = normalize(&Value::Array(snapshot(&root)), &root, "", new_ns);
        let rows = normalize(&c.rows(), &root, "", new_ns);
        let label = format!(
            "{} step {i} ({})",
            case["name"],
            s.get("tool").or(s.get("fn")).unwrap_or(&json!("op"))
        );
        if ret != s["ret"] {
            diffs.push(format!(
                "{label}: result\n  rust:   {ret}\n  python: {}",
                s["ret"]
            ));
        }
        let want = if s["tree"].is_null() {
            last.clone()
        } else {
            s["tree"].clone()
        };
        if tree != want {
            diffs.push(format!("{label}: tree\n  rust:   {tree}\n  python: {want}"));
        }
        last = want;
        if rows != s["rows"] {
            diffs.push(format!(
                "{label}: rows\n  rust:   {rows}\n  python: {}",
                s["rows"]
            ));
        }
    }
    clear_flags(&root);
    diffs
}

fn run_case(name: &str) {
    setup();
    let f = fixture();
    let old_ns = f["old_ns"].as_i64().unwrap() as i128;
    let new_ns = f["new_ns"].as_i64().unwrap();
    let case = f["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .expect(name);
    let diffs = play(case, old_ns, new_ns);
    assert!(
        diffs.is_empty(),
        "{} differences:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

macro_rules! cases {
    ($($test:ident => $name:literal),+ $(,)?) => {
        $(#[test] fn $test() { run_case($name); })+

        #[test]
        fn every_case_is_replayed() {
            let f = fixture();
            let named: HashSet<&str> = [$($name),+].into_iter().collect();
            for c in f["cases"].as_array().unwrap() {
                assert!(named.contains(c["name"].as_str().unwrap()), "no test replays {}", c["name"]);
            }
        }
    };
}

cases! {
    relative_and_absolute => "relative_and_absolute",
    symlinked_folder => "symlinked_folder",
    overwrite_and_edit => "overwrite_and_edit",
    edit_line_endings => "edit_line_endings",
    read_pages => "read_pages",
    list_pages => "list_pages",
    write_keeps_what_the_owner_had => "write_keeps_what_the_owner_had",
    hardlink_race => "hardlink_race",
    locked_needs_permission => "locked_needs_permission",
    protection_lists => "protection_lists",
    copy_and_move => "copy_and_move",
    trash => "trash",
    trash_folder_made => "trash_folder_made",
    trash_blocked => "trash_blocked",
    trash_not_a_trash => "trash_not_a_trash",
    disk_usage => "disk_usage",
    look => "look",
    recovery_copies => "recovery_copies",
    recovery_trash => "recovery_trash",
    recovery_sweep => "recovery_sweep",
    copy_into => "copy_into",
    snapshots => "snapshots",
    snapshots_off => "snapshots_off",
    no_clones => "no_clones",
}

/// local_snapshot's reading of tmutil's answer (HAND-408: only the strict form is FERB's own name).
#[test]
fn tmutil_answers_read_as_python_read_them() {
    let f = fixture();
    for c in f["tmutil"].as_array().unwrap() {
        let got = recovery::snapshot_from_output(
            c["code"].as_i64().unwrap() as i32,
            c["stdout"].as_str().unwrap(),
            c["stderr"].as_str().unwrap(),
        );
        assert_eq!(json!(got), c["name"], "{c}");
    }
}

#[test]
fn utc_times_round_as_python_does() {
    // datetime.fromtimestamp(t, timezone.utc).isoformat(): round half to even at the microsecond
    assert_eq!(
        files::utc_iso(978_307_200.123_456_8),
        "2001-01-01T00:00:00.123457+00:00"
    );
    assert_eq!(files::utc_iso(0.0), "1970-01-01T00:00:00+00:00");
    assert_eq!(
        files::utc_iso(1.000_000_5),
        "1970-01-01T00:00:01.000001+00:00"
    );
    let _ = FixedOffset::east_opt(0).unwrap().timestamp_opt(0, 0);
}

// ---------- what the tools declare to the gateway ----------

fn fail_json(f: files::Fail) -> Value {
    match f {
        files::Fail::Raised(t) => json!({"raised": "RuntimeError", "text": t}),
        files::Fail::Refused(t) => json!({"raised": "PathRefused", "text": t}),
        files::Fail::Os(e) => json!({"raised": usage::error_class(&e), "text": e.to_string()}),
        files::Fail::Decode(n) => json!({"raised": "UnicodeDecodeError", "text": n}),
    }
}

fn val<T: serde::Serialize>(r: Result<T, files::Fail>) -> Value {
    match r {
        Ok(v) => serde_json::to_value(v).unwrap(),
        Err(f) => fail_json(f),
    }
}

/// describe, file_paths, locks, real_path and recovery_reason as the Python tools declare them.
/// decisions: GATE-539, HAND-432
#[test]
fn declarations_match_python() {
    use files::declared as d;
    setup();
    let f = fixture();
    let keep = tempfile::tempdir().unwrap();
    let base = ferb_fs::pypath::realpath(keep.path().to_str().unwrap());
    let root = format!("{base}/declared");
    let tree: Vec<Value> = serde_json::from_value(json!([
        ["dir", "home/.ferb/workspace", 0o755],
        ["file", "home/Docs/a.txt", "a", 0o644, {}],
        ["link", "home/Docs/link", "../Docs"],
        ["link", "home/Docs/file-link", "a.txt"]
    ]))
    .unwrap();
    build(&root, &tree, 0);
    let home = format!("{root}/home");
    let ws = format!("{home}/.ferb/workspace");
    let new_ns = f["new_ns"].as_i64().unwrap();
    let mut diffs = Vec::new();
    for want in f["declared"].as_array().unwrap() {
        let got = if let Some(p) = want.get("normalize_path") {
            let p = subst(p.as_str().unwrap(), &root);
            json!({"normalize_path": p, "is": val(files::normalize_path(&p, &ws, &home))})
        } else {
            let args = Value::Object(
                want["args"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str()
                                .map(|s| json!(subst(s, &root)))
                                .unwrap_or(v.clone()),
                        )
                    })
                    .collect(),
            );
            let path = args["path"].as_str().unwrap().to_string();
            let tool = want["tool"].as_str().unwrap();
            let (describe, file_paths, locks, real, reason): (Value, Value, Value, Value, Value) =
                match tool {
                    "fs.list" => (
                        val(d::list_describe(
                            &serde_json::from_value(args.clone()).unwrap(),
                            &home,
                        )),
                        json!(d::file_paths(&path, &files::FS_LIST)),
                        val(d::locks(&path, &home)),
                        val(d::real(&path, &ws, &home)),
                        json!("<none>"),
                    ),
                    "fs.read" => (
                        val(d::read_describe(
                            &serde_json::from_value(args.clone()).unwrap(),
                            &home,
                        )),
                        json!(d::file_paths(&path, &files::FS_READ)),
                        val(d::locks(&path, &home)),
                        val(d::real(&path, &ws, &home)),
                        json!("<none>"),
                    ),
                    "fs.write" => {
                        let a = serde_json::from_value(args.clone()).unwrap();
                        (
                            val(d::write_describe(&a, &home)),
                            json!(d::file_paths(&path, &files::FS_WRITE)),
                            val(d::locks(&path, &home)),
                            val(d::real(&path, &ws, &home)),
                            json!(d::write_reason(&a)),
                        )
                    }
                    "fs.edit" => {
                        let a = serde_json::from_value(args.clone()).unwrap();
                        (
                            val(d::edit_describe(&a, &home)),
                            json!(d::file_paths(&path, &files::FS_EDIT)),
                            val(d::locks(&path, &home)),
                            val(d::real(&path, &ws, &home)),
                            json!(d::edit_reason(&a)),
                        )
                    }
                    "fs.trash" => {
                        let a = serde_json::from_value(args.clone()).unwrap();
                        (
                            val(d::trash_describe(&a, &home)),
                            json!(d::file_paths(&path, &files::FS_TRASH)),
                            val(d::locks(&path, &home)),
                            val(d::trash_real(&a, &ws, &home)),
                            json!("<none>"),
                        )
                    }
                    "fs.copy" => {
                        let a: files::CopyArgs = serde_json::from_value(args.clone()).unwrap();
                        (
                            val(d::copy_describe(&a, &home)),
                            json!(d::copy_file_paths(&a)),
                            val(d::two_locks(&a.path, &a.destination, &home)),
                            val(d::real(&a.destination, &ws, &home)),
                            json!(d::copy_reason(&a)),
                        )
                    }
                    "fs.move" => {
                        let a: files::MoveArgs = serde_json::from_value(args.clone()).unwrap();
                        (
                            val(d::move_describe(&a, &home)),
                            json!(d::move_file_paths(&a)),
                            val(d::two_locks(&a.path, &a.destination, &home)),
                            val(d::move_real(&a, &ws, &home)),
                            json!(d::move_reason(&a)),
                        )
                    }
                    other => panic!("{other}"),
                };
            json!({"tool": tool, "args": want["args"], "describe": describe, "file_paths": file_paths,
                   "locks": locks, "real_path": real, "recovery_reason": reason})
        };
        let got = normalize(&got, &root, "", new_ns);
        if &got != want {
            diffs.push(format!("rust:   {got}\npython: {want}"));
        }
    }
    assert!(
        diffs.is_empty(),
        "{} differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// The file-identity half of protection.py: each path as typed, as resolved, as the kernel names it, and how a volume
/// compares names.
#[test]
fn protection_identity_matches_python() {
    use ferb_fs::identity as id;
    setup();
    let f = fixture();
    let keep = tempfile::tempdir().unwrap();
    let base = ferb_fs::pypath::realpath(keep.path().to_str().unwrap());
    let root = format!("{base}/identity");
    let tree: Vec<Value> = serde_json::from_value(json!([
        ["file", "home/Docs/a.txt", "a", 0o644, {}],
        ["link", "home/Docs/link", "../Private"],
        ["dir", "home/Private/inner", 0o755],
        ["link", "home/dangling", "nowhere/x"],
        ["fifo", "home/Docs/pipe"],
        ["dir", "home/Ünï/Çase", 0o755],
        ["dir", "home/shut/in", 0o755],
        ["link", "home/loop", "loop"]
    ]))
    .unwrap();
    build(&root, &tree, 0);
    fs::set_permissions(
        format!("{root}/home/shut"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let home = format!("{root}/home");
    let top: Vec<String> = id::key(&root)[1..].to_vec();
    let rooted = |k: Vec<String>| -> Vec<String> {
        for i in 0..k.len() {
            if k.len() >= i + top.len() && k[i..i + top.len()] == top[..] {
                return [&k[..i], &["$ROOT".to_string()], &k[i + top.len()..]].concat();
            }
        }
        k
    };
    let mut diffs = Vec::new();
    for want in f["identity"].as_array().unwrap() {
        let got = if let Some(pair) = want.get("under") {
            let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            json!({"under": pair, "is": id::under(a, b)})
        } else {
            let p = subst(want["path"].as_str().unwrap(), &root);
            let got = json!({"path": p, "typed": id::typed(&p), "real": id::real(&p, &home), "kernel": id::kernel(&p),
                   "firmlinked": id::firmlinked(&p), "forms": id::forms(&p, &home), "unmapped": id::unmapped(&p),
                   "key": rooted(id::key(&p))});
            normalize(&got, &root, "", i64::MAX)
        };
        if &got != want {
            diffs.push(format!("rust:   {got}\npython: {want}"));
        }
    }
    fs::set_permissions(
        format!("{root}/home/shut"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(
        diffs.is_empty(),
        "{} differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// new_file (never replacing anything, through the opened folder) and destination_closed.
#[test]
fn new_file_and_destination_check_match_python() {
    setup();
    let f = fixture();
    let keep = tempfile::tempdir().unwrap();
    let base = ferb_fs::pypath::realpath(keep.path().to_str().unwrap());
    let root = format!("{base}/newfile");
    let tree: Vec<Value> = serde_json::from_value(json!([
        ["file", "home/Docs/a.txt", "a", 0o644, {}],
        ["link", "home/Docs/link", "a.txt"],
        ["dir", "home/Docs/dir", 0o755],
        ["link", "home/sneaky", "Docs"]
    ]))
    .unwrap();
    let old_ns = f["old_ns"].as_i64().unwrap() as i128;
    build(&root, &tree, old_ns);
    let r2 = root.clone();
    let say = move |p: &str| {
        p.contains("/Docs/dir")
            .then(|| format!("closed {}", p.replace(&r2, "$ROOT")))
    };
    let entries = f["new_file"].as_array().unwrap();
    let new_ns = f["new_ns"].as_i64().unwrap();
    let mut diffs = Vec::new();
    for want in &entries[..entries.len() - 1] {
        let rel = want["path"].as_str().unwrap();
        let data = match rel {
            "home/Docs/new.pdf" => "%PDF new",
            _ => "x",
        };
        let p = format!("{root}/{rel}");
        let ret = match files::new_file(&p, data.as_bytes(), Some(&say)) {
            Ok(()) => Value::Null,
            Err(e) => fail_json(e),
        };
        let dest = files::destination_closed(&format!("{p}/deeper/z"), Some(&say)).unwrap();
        let got = normalize(
            &json!({"path": rel, "ret": ret, "destination": dest}),
            &root,
            "",
            new_ns,
        );
        if &got != want {
            diffs.push(format!("rust:   {got}\npython: {want}"));
        }
    }
    let tree = normalize(&Value::Array(snapshot(&root)), &root, "", new_ns);
    if tree != entries[entries.len() - 1]["tree"] {
        diffs.push(format!(
            "tree:\n  rust:   {tree}\n  python: {}",
            entries[entries.len() - 1]["tree"]
        ));
    }
    assert!(
        diffs.is_empty(),
        "{} differ:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}

/// The names, descriptions, scopes, risk and reversibility of each tool as the frozen catalog has them.
#[test]
fn tool_facts_match_the_frozen_catalog() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/catalog/tools.json");
    let catalog: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let tools: HashMap<String, Value> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t.clone()))
        .collect();
    for info in [
        files::FS_LIST,
        files::FS_READ,
        files::FS_WRITE,
        files::FS_EDIT,
        files::FS_COPY,
        files::FS_MOVE,
        files::FS_TRASH,
    ] {
        let t = &tools[info.name];
        assert_eq!(t["description"], json!(info.description), "{}", info.name);
        assert_eq!(t["scopes"], json!(info.scopes), "{}", info.name);
        assert_eq!(t["facts"]["risk"], json!(info.risk as u8), "{}", info.name);
        assert_eq!(
            t["facts"]["reversible"],
            json!(info.reversible),
            "{}",
            info.name
        );
        assert_eq!(
            t["facts"]["target_args"],
            json!(info.target_args),
            "{}",
            info.name
        );
        assert_eq!(t["identities"], json!(["ferb", "owner"]), "{}", info.name);
    }
    assert_eq!(tools[look::NAME]["description"], json!(look::DESCRIPTION));
    assert_eq!(tools[usage::NAME]["description"], json!(usage::DESCRIPTION));
    assert_eq!(
        tools[recovery::SNAPSHOT_TOOL]["description"],
        json!(recovery::SNAPSHOT_DESCRIPTION)
    );
}
