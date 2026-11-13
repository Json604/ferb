//! downloads.py's `Downloads` against rust/fixtures/browser/downloads.json: the Python class was driven with a
//! scripted extension peer (tests/fake_extension.py's download_part rules), a fake clock, fake free space and the
//! real store; here the same steps run against the Rust port with the same peer rules, and every peer call, published
//! event, cap check, stored row, shell count and file left must be the same. Real time (a slow part, the wait for a
//! tab's lock, a settle) is Python's scaled by `SCALE` on tokio's paused clock.
#![cfg(target_os = "macos")]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ferb_browser::bridge::TaskTab;
use ferb_browser::downloads::{
    self, sqlite::SqliteRecords, DlHooks, Downloads, Fault, Peer, Records, Row,
};
use ferb_browser::protocol::Event;
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::when::DateTime;
use serde_json::{json, Map, Value};

use common::{fixture, Counter};

type Log = Arc<Mutex<Vec<Value>>>;

struct FakeClock(Mutex<(DateTime, f64)>);

impl FakeClock {
    fn advance(&self, s: f64) {
        let mut c = self.0.lock().unwrap();
        c.0 += chrono::Duration::microseconds((s * 1e6).round() as i64);
        c.1 += s;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        self.0.lock().unwrap().0
    }
    fn monotonic(&self) -> f64 {
        self.0.lock().unwrap().1
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// The scripted extension side of `download_part` (more.py's `Peer`).
#[derive(Default)]
struct PeerState {
    held: HashMap<String, Vec<u8>>,
    tick: f64,
    empty_parts: i64,
    silent_parts: i64,
    fail_at: Option<i64>,
    wrong_offset: bool,
    delay: f64,
    refuse: bool,
}

struct TestPeer {
    name: String,
    state: Arc<Mutex<PeerState>>,
    calls: Log,
    clock: Arc<FakeClock>,
    part: usize,
    scale: f64,
}

impl Peer for TestPeer {
    fn name(&self) -> String {
        self.name.clone()
    }
    fn call(
        &self,
        op: &str,
        args: Value,
        timeout: f64,
    ) -> BoxFuture<'static, Result<Value, String>> {
        self.calls
            .lock()
            .unwrap()
            .push(json!([self.name, op, args, timeout]));
        let (state, clock, part, scale) = (
            self.state.clone(),
            self.clock.clone(),
            self.part,
            self.scale,
        );
        Box::pin(async move {
            let delay = state.lock().unwrap().delay;
            if delay > 0.0 {
                tokio::time::sleep(std::time::Duration::from_secs_f64(delay * scale)).await;
            } else {
                tokio::task::yield_now().await;
            }
            let mut s = state.lock().unwrap();
            let did = args["download_id"].as_str().unwrap().to_string();
            if s.refuse {
                return Err("the connection to the browser closed".into());
            }
            if !s.held.contains_key(&did) {
                return Err("download transfer unavailable".into());
            }
            if args.get("release").is_some() {
                s.held.remove(&did);
                return Ok(json!({"released": true, "handed_back": true}));
            }
            if args.get("abort").is_some() {
                s.held.remove(&did);
                return Ok(json!({"aborted": true}));
            }
            if s.tick != 0.0 {
                clock.advance(s.tick);
            }
            if s.silent_parts > 0 {
                s.silent_parts -= 1;
                clock.advance(timeout);
                return Err(String::new());
            }
            let offset = args["offset"].as_i64().unwrap();
            if s.empty_parts > 0 {
                s.empty_parts -= 1;
                return Ok(json!({"data": "", "offset": offset, "eof": false}));
            }
            if s.fail_at.is_some_and(|f| offset >= f) {
                s.held.remove(&did);
                return Err("the response stream broke".into());
            }
            if s.wrong_offset {
                return Ok(json!({"data": "", "offset": offset + 1, "eof": false}));
            }
            let data = s.held[&did].clone();
            let start = (offset as usize).min(data.len());
            let chunk = &data[start..(start + part).min(data.len())];
            let eof = start + chunk.len() >= data.len();
            if eof {
                s.held.remove(&did);
            }
            Ok(json!({"data": ferb_identity::py::b64encode(chunk), "offset": offset, "eof": eof}))
        })
    }
}

/// A store whose `taken` update fails (Python's test patching `Downloads._finish`).
struct Flaky {
    inner: SqliteRecords,
    broken: Arc<Mutex<bool>>,
}

impl Records for Flaky {
    fn insert(&self, row: &Row) -> Result<(), Fault> {
        self.inner.insert(row)
    }
    fn finish(
        &self,
        id: &str,
        state: &str,
        path: Option<&str>,
        name: Option<&str>,
        size: Option<i64>,
        sha256: Option<&str>,
        reason: Option<&str>,
    ) -> Result<Row, Fault> {
        if *self.broken.lock().unwrap() && state == "taken" {
            return Err(Fault::new("OperationalError", "disk I/O error"));
        }
        self.inner
            .finish(id, state, path, name, size, sha256, reason)
    }
    fn tree_usage(&self, tree: &str) -> Result<(i64, i64), Fault> {
        self.inner.tree_usage(tree)
    }
    fn shell_bytes(&self, tree: &str) -> Result<Option<i64>, Fault> {
        self.inner.shell_bytes(tree)
    }
    fn set_shell(&self, tree: &str, bytes: i64, at: &str) -> Result<(), Fault> {
        self.inner.set_shell(tree, bytes, at)
    }
    fn finalize_stale(&self) -> Result<i64, Fault> {
        self.inner.finalize_stale()
    }
    fn with_path(&self) -> Result<Vec<Row>, Fault> {
        self.inner.with_path()
    }
    fn set_kept(&self, id: &str, kept: &str) -> Result<(), Fault> {
        self.inner.set_kept(id, kept)
    }
    fn clear_path(&self, id: &str, kept: &str) -> Result<(), Fault> {
        self.inner.clear_path(id, kept)
    }
    fn of_tasks(&self, tasks: &[String], identity: Option<&str>) -> Result<Vec<Row>, Fault> {
        self.inner.of_tasks(tasks, identity)
    }
}

fn blob(n: usize, seed: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + seed) % 256) as u8).collect()
}

fn select(conn: &rusqlite::Connection, sql: &str) -> Vec<Value> {
    let mut st = conn.prepare(sql).unwrap();
    let names: Vec<String> = st.column_names().map(String::from).collect();
    let mut q = st.query([]).unwrap();
    let mut out = vec![];
    while let Some(r) = q.next().unwrap() {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            let v = match r.get_ref(i).unwrap() {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(i) => json!(i),
                rusqlite::types::ValueRef::Real(f) => json!(f),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(_) => panic!("blob"),
            };
            m.insert(n.clone(), v);
        }
        out.push(Value::Object(m));
    }
    out
}

fn files(root: &Path) -> Value {
    let dir = root.join("ws/downloads");
    if !dir.is_dir() {
        return Value::Null;
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut out = vec![];
    for name in names {
        let p = dir.join(&name);
        let m = std::fs::symlink_metadata(&p).unwrap();
        if m.file_type().is_symlink() {
            out.push(json!([
                name,
                "link",
                std::fs::read_link(&p).unwrap().to_string_lossy()
            ]));
        } else if m.is_file() {
            use sha2::Digest;
            use std::os::unix::fs::PermissionsExt;
            let data = std::fs::read(&p).unwrap();
            let sha: String = sha2::Sha256::digest(&data)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            out.push(json!([
                name,
                data.len(),
                sha,
                m.permissions().mode() & 0o7777
            ]));
        } else {
            out.push(json!([name, "dir"]));
        }
    }
    json!(out)
}

/// A recorded call's timeout as a float (Python sent `timeout=10` for an abort, an int).
fn calls_shape(v: &Value) -> Value {
    json!(v
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let mut c = c.clone();
            c[3] = json!(c[3].as_f64());
            c
        })
        .collect::<Vec<_>>())
}

fn fault(e: Fault) -> Value {
    json!({"error": e.class, "message": e.message})
}

/// Scenarios whose transfers run in two tabs at once.
const INTERLEAVED: &[&str] = &["two_tabs_share_the_budget"];

async fn replay(f: &Value, scenario: &Value, random: &Arc<Counter>) -> usize {
    let name = scenario["name"].as_str().unwrap().to_string();
    let scale = 20.0 / f["tab_wait_s"].as_f64().unwrap();
    let part = f["part"].as_u64().unwrap() as usize;
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().canonicalize().unwrap();
    let r = |s: &str| s.replace("$ROOT", &root.to_string_lossy());
    let unroot = |v: Value| -> Value {
        serde_json::from_str(&v.to_string().replace(&*root.to_string_lossy(), "$ROOT")).unwrap()
    };
    let path = root.join("ferb.db");
    let db = ferb_store::Db::open(&path).unwrap();
    db.migrate().unwrap();
    db.close().unwrap();
    let reader = rusqlite::Connection::open(&path).unwrap();
    let broken = Arc::new(Mutex::new(false));
    let records = Arc::new(Flaky {
        inner: SqliteRecords::new(rusqlite::Connection::open(&path).unwrap()),
        broken: broken.clone(),
    });
    let start = ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap();
    let clock = Arc::new(FakeClock(Mutex::new((start, 1000.0))));
    let config: Arc<Mutex<Map<String, Value>>> = Arc::default();
    let c = config.clone();
    let pubs: Log = Arc::default();
    let p = pubs.clone();
    let d = Downloads::new(
        root.join("ws/downloads"),
        records,
        clock.clone(),
        Arc::new(move |section: &str, key: &str| {
            assert_eq!(section, "browser");
            let c = c.lock().unwrap();
            if c.get("broken") == Some(&json!(true)) {
                return Err(Fault::new("ValueError", "config.toml is half-edited"));
            }
            Ok(c.get(key).cloned())
        }),
        Arc::new(|t: &str| t.replace("SECRET", "[redacted]")),
        Arc::new(move |topic: &str, data: Map<String, Value>| {
            p.lock().unwrap().push(json!([topic, data]))
        }),
        random.clone(),
    );
    d.set_tab_wait(20.0);
    let tabs: Arc<Mutex<HashMap<(String, i64), TaskTab>>> = Arc::default();
    let free: Arc<Mutex<Value>> = Arc::new(Mutex::new(json!(1_000_000_000_000_000i64)));
    let free_after: Arc<Mutex<Vec<Value>>> = Arc::default();
    let hooks_log: Log = Arc::default();
    let me: Arc<OnceLock<std::sync::Weak<Downloads>>> = Arc::default();
    let (t, fr, fa, hl, m2) = (
        tabs.clone(),
        free.clone(),
        free_after.clone(),
        hooks_log.clone(),
        me.clone(),
    );
    me.set(Arc::downgrade(&d)).unwrap();
    d.set_hooks(DlHooks {
        task_tab: Arc::new(move |peer: &dyn Peer, tab: i64| {
            t.lock().unwrap().get(&(peer.name(), tab)).cloned()
        }),
        task_root: Arc::new(|task: &str| task.split('.').next().unwrap().to_string()),
        free_bytes: Arc::new(move || {
            let mut after = fa.lock().unwrap();
            if !after.is_empty() {
                if after[0] == json!(0) {
                    after.remove(0);
                    *fr.lock().unwrap() = after.remove(0);
                } else {
                    after[0] = json!(after[0].as_i64().unwrap() - 1);
                }
            }
            let v = fr.lock().unwrap().clone();
            match v.as_i64() {
                Some(n) => Ok(n),
                None => Err("[Errno 5] Input/output error".into()),
            }
        }),
        on_over_cap: Arc::new(move || {
            let d = m2.get().unwrap().upgrade().unwrap();
            let o = d.over_cap().unwrap();
            hl.lock().unwrap().push(json!([
                "over_cap",
                o.map(|o| json!([o.ids, o.bytes, o.total, o.cap]))
            ]));
        }),
    });
    let calls: Log = Arc::default();
    let mut peers: HashMap<String, Arc<TestPeer>> = HashMap::new();
    let mut peer = |n: &str| -> Arc<TestPeer> {
        peers
            .entry(n.to_string())
            .or_insert_with(|| {
                Arc::new(TestPeer {
                    name: n.into(),
                    state: Arc::default(),
                    calls: calls.clone(),
                    clock: clock.clone(),
                    part,
                    scale,
                })
            })
            .clone()
    };
    let mut last: Map<String, Value> = Map::new();
    let mut compared = 0;
    for (n, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
        let at = format!("{name} step {n}");
        let s = &step["step"];
        let (kind, a) = s.as_object().unwrap().iter().next().unwrap();
        let (c0, p0, h0) = (
            calls.lock().unwrap().len(),
            pubs.lock().unwrap().len(),
            hooks_log.lock().unwrap().len(),
        );
        let rows_now = || select(&reader, "SELECT * FROM downloads ORDER BY rowid");
        let row_of = |text: &str| -> String {
            let rows = rows_now();
            if let Some(i) = text.strip_prefix("$row-") {
                return rows[i.parse::<usize>().unwrap()]["id"]
                    .as_str()
                    .unwrap()
                    .into();
            }
            if let Some(i) = text.strip_prefix("$path-") {
                return rows[i.parse::<usize>().unwrap()]["path"]
                    .as_str()
                    .unwrap()
                    .into();
            }
            r(text)
        };
        let mut got = json!({"ok": null});
        match kind.as_str() {
            "tab" => {
                tabs.lock().unwrap().insert(
                    (a[0].as_str().unwrap().into(), a[1].as_i64().unwrap()),
                    TaskTab {
                        task: a[2].as_str().unwrap().into(),
                        tab: a[3].as_str().unwrap().into(),
                        identity: a[4].as_str().unwrap().into(),
                    },
                );
            }
            "peer" => {
                let p = peer(a["name"].as_str().unwrap());
                let mut st = p.state.lock().unwrap();
                for (k, v) in a.as_object().unwrap() {
                    match k.as_str() {
                        "name" => {}
                        "tick" => st.tick = v.as_f64().unwrap(),
                        "empty_parts" => st.empty_parts = v.as_i64().unwrap(),
                        "silent_parts" => st.silent_parts = v.as_i64().unwrap(),
                        "fail_at" => st.fail_at = v.as_i64(),
                        "wrong_offset" => st.wrong_offset = v.as_bool().unwrap(),
                        "delay" => st.delay = v.as_f64().unwrap(),
                        "refuse" => st.refuse = v.as_bool().unwrap(),
                        other => panic!("{at}: peer {other}"),
                    }
                }
            }
            "config" => *config.lock().unwrap() = a.as_object().unwrap().clone(),
            "free" => *free.lock().unwrap() = a.clone(),
            "free_after" => *free_after.lock().unwrap() = a.as_array().unwrap().clone(),
            "broken_finish" => *broken.lock().unwrap() = a.as_bool().unwrap(),
            "event" => {
                let p = peer(
                    step["step"]
                        .get("peer")
                        .and_then(Value::as_str)
                        .unwrap_or("chrome/Profile 1"),
                );
                if let (Some(data), Some(id)) =
                    (s.get("data"), a.get("download_id").and_then(Value::as_str))
                {
                    let d = data.as_array().unwrap();
                    p.state.lock().unwrap().held.insert(
                        id.into(),
                        blob(
                            d[0].as_u64().unwrap() as usize,
                            d[1].as_u64().unwrap() as usize,
                        ),
                    );
                }
                let mut m = a.as_object().unwrap().clone();
                m.insert("event".into(), "download".into());
                let mut full = Map::new();
                full.insert("event".into(), "download".into());
                full.extend(m);
                let ev = Event::validate(&full).unwrap_or_else(|_| panic!("{at}: the event"));
                d.receive(p, ev);
            }
            "wait" => {
                d.idle().await;
                for _ in 0..20 {
                    tokio::task::yield_now().await;
                }
            }
            "settle" => {
                d.settle_in(
                    a[0].as_str().unwrap(),
                    a[1].as_i64().unwrap(),
                    a[2].as_f64().unwrap() * scale,
                )
                .await
            }
            "busy" => {
                got = json!({"ok": d.busy_in(a[0].as_str().unwrap(), a[1].as_i64().unwrap())})
            }
            "unreported" => {
                got = json!({"ok": d.unreported_of(a[0].as_str().unwrap(), a[1].as_i64().unwrap())})
            }
            "facts_for" => {
                let tasks: Vec<String> = serde_json::from_value(a[0].clone()).unwrap();
                got = match d.facts_for(&tasks, a[1].as_str()) {
                    Ok(v) => json!({"ok": v}),
                    Err(e) => fault(e),
                };
            }
            "shell_received" => {
                if let Err(e) = d.shell_received(a[0].as_str().unwrap(), a[1].as_i64().unwrap()) {
                    got = fault(e);
                }
            }
            "shell_stop" => {
                got = match d.shell_stop(a.as_str().unwrap()) {
                    Ok(v) => json!({"ok": v}),
                    Err(e) => fault(e),
                }
            }
            "forget_cache" => d.forget_shell_cache(),
            "limits" => {
                got = match d.limits() {
                    Ok(l) => {
                        json!({"ok": [l.task_files, l.task_bytes, l.total_bytes, l.min_free_bytes]})
                    }
                    Err(e) => fault(e),
                }
            }
            "advance" => clock.advance(a.as_f64().unwrap()),
            "over_cap" => {
                got = match d.over_cap() {
                    Ok(o) => json!({"ok": o.map(|o| json!([o.ids, o.bytes, o.total, o.cap]))}),
                    Err(e) => fault(e),
                }
            }
            "delete" => {
                let ids: Vec<String> = a
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| row_of(x.as_str().unwrap()))
                    .collect();
                got = match d.delete(&ids) {
                    Ok((c, b)) => json!({"ok": [c, b]}),
                    Err(e) => fault(e),
                }
            }
            "fs" => {
                let target = row_of(a[1].as_str().unwrap());
                let p = if target.starts_with('/') {
                    PathBuf::from(&target)
                } else {
                    root.join(&target)
                };
                match a[0].as_str().unwrap() {
                    "unlink" => std::fs::remove_file(&p).unwrap(),
                    "symlink" => {
                        std::os::unix::fs::symlink(row_of(a[2].as_str().unwrap()), &p).unwrap()
                    }
                    "write" => {
                        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                        std::fs::write(&p, a[2].as_str().unwrap()).unwrap();
                    }
                    "mkdir" => std::fs::create_dir_all(&p).unwrap(),
                    other => panic!("{at}: fs {other}"),
                }
            }
            "sql" => {
                reader.execute(&r(a.as_str().unwrap()), []).unwrap();
            }
            "sweep" => {
                got = match d.sweep() {
                    Ok(n) => json!({"ok": n}),
                    Err(e) => json!({"error": "OSError", "message": e}),
                }
            }
            "finalize_stale" => {
                got = match d.finalize_stale() {
                    Ok(n) => json!({"ok": n}),
                    Err(e) => fault(e),
                }
            }
            "forget_task" => d.forget_task(a.as_str().unwrap()),
            "internal" => got = json!({"ok": d.counters()}),
            other => panic!("{at}: unknown step {other}"),
        }
        let got = unroot(got);
        assert_eq!(got, step["out"], "{at}: {s}");
        let made = calls_shape(&unroot(json!(calls.lock().unwrap()[c0..])));
        let events = unroot(json!(pubs.lock().unwrap()[p0..]));
        if INTERLEAVED.contains(&name.as_str()) {
            // Two tabs' transfers run at once: which one's part goes first is each runtime's scheduling (asyncio's
            // and tokio's differ), not a contract; the same calls and events must happen, and the same outcome.
            let sorted = |v: &Value| {
                let mut a: Vec<String> =
                    v.as_array().unwrap().iter().map(Value::to_string).collect();
                a.sort();
                a
            };
            assert_eq!(
                sorted(&made),
                sorted(&calls_shape(&step["calls"])),
                "{at}: peer calls after {s}"
            );
            assert_eq!(
                sorted(&events),
                sorted(&step["pub"]),
                "{at}: events after {s}"
            );
        } else {
            assert_eq!(
                made,
                calls_shape(&step["calls"]),
                "{at}: peer calls after {s}"
            );
            assert_eq!(events, step["pub"], "{at}: events after {s}");
        }
        assert_eq!(
            json!(hooks_log.lock().unwrap()[h0..]),
            step["hooks"],
            "{at}: cap checks after {s}"
        );
        let now = [
            ("rows", unroot(json!(rows_now()))),
            (
                "shell",
                json!(select(
                    &reader,
                    "SELECT * FROM shell_downloads ORDER BY tree_id"
                )),
            ),
            ("files", unroot(files(&root))),
        ];
        for (k, v) in now {
            let want = step
                .get(k)
                .cloned()
                .unwrap_or_else(|| last.get(k).cloned().unwrap_or(Value::Null));
            assert_eq!(v, want, "{at}: {k} after {s}");
            last.insert(k.into(), want);
        }
        compared += 7;
    }
    compared
}

// decisions: HAND-210, HAND-211, HAND-231, HAND-233, HAND-235, HAND-236, HAND-242
#[tokio::test(start_paused = true)]
async fn downloads_match_python() {
    let f = fixture("downloads.json");
    // one counter for the whole file, as make.py resets it once before downloads_fixture
    let random = Arc::new(Counter::new());
    let mut total = 0;
    for scenario in f["scenarios"].as_array().unwrap() {
        total += replay(&f, scenario, &random).await;
    }
    assert!(total > 1000, "{total}");
}

#[test]
fn the_pure_parts_match_python() {
    let f = fixture("downloads.json");
    let p = &f["pure"];
    for (raw, want) in p["safe_name"].as_object().unwrap() {
        assert_eq!(downloads::safe_name(raw), want.as_str().unwrap(), "{raw:?}");
    }
    for (n, want) in p["gb"].as_object().unwrap() {
        assert_eq!(
            downloads::gb(n.parse().unwrap()),
            want.as_str().unwrap(),
            "{n}"
        );
    }
    let roots = p["untrusted_roots"].as_array().unwrap();
    let cases: [&[&str]; 4] = [
        &["~/Downloads"],
        &[],
        &["/ws/downloads", "/x"],
        &["/ws/uploads/outside", "/ws/downloads"],
    ];
    for (case, want) in cases.iter().zip(roots) {
        let configured: Vec<String> = case.iter().map(|s| s.to_string()).collect();
        assert_eq!(json!(downloads::untrusted_roots(&configured, "/ws")), *want);
    }
    for (d, want) in p["not_taken"].as_object().unwrap() {
        let detail = (d != "None").then_some(d.as_str());
        assert_eq!(downloads::not_taken(detail), want.as_str().unwrap());
    }
    let defaults: Map<String, Value> = downloads::DEFAULTS
        .iter()
        .map(|(k, v)| (k.to_string(), json!(*v as i64)))
        .collect();
    assert_eq!(Value::Object(defaults), p["defaults"]);
}

#[test]
fn free_space_is_read_and_falls_back_to_statvfs() {
    let dir = tempfile::tempdir().unwrap();
    let important = |_: &Path| Some(42i64);
    assert_eq!(
        downloads::available_bytes(dir.path(), Some(&important)),
        Ok(42)
    );
    let none = |_: &Path| None;
    assert!(downloads::available_bytes(dir.path(), Some(&none)).unwrap() > 0);
    let err = downloads::available_bytes(&dir.path().join("missing"), None).unwrap_err();
    assert!(
        err.starts_with("[Errno 2] No such file or directory: '"),
        "{err}"
    );
}

/// ferb-shell's download watch through `shell_hook`: received bytes count against the tree's budget, and the command
/// that passes it is told why (journaled); the row is kept for a restart (HAND-210, HAND-231, HAND-232).
#[tokio::test]
async fn the_shell_watch_counts_against_the_same_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ferb.db");
    let db = ferb_store::Db::open(&path).unwrap();
    db.migrate().unwrap();
    db.close().unwrap();
    let start = ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap();
    let d = Downloads::new(
        dir.path().join("ws/downloads"),
        Arc::new(SqliteRecords::new(
            rusqlite::Connection::open(&path).unwrap(),
        )),
        Arc::new(FakeClock(Mutex::new((start, 1000.0)))),
        Arc::new(|_: &str, k: &str| Ok((k == "download_task_max_gb").then(|| json!(0.000001)))),
        Arc::new(str::to_string),
        Arc::new(|_: &str, _: Map<String, Value>| {}),
        Arc::new(Counter::new()),
    );
    let mut hooks = d.hooks();
    hooks.task_root = Arc::new(|task: &str| task.split('.').next().unwrap().to_string());
    hooks.free_bytes = Arc::new(|| Ok(1_000_000_000_000_000));
    d.set_hooks(hooks);
    let told: Log = Arc::default();
    let t = told.clone();
    let hook = d.shell_hook(Arc::new(move |task: &str, why: &str| {
        t.lock().unwrap().push(json!([task, why]))
    }));
    assert_eq!(hook("t1.step2", 600), None);
    assert_eq!(hook("t1.step3", 0), None);
    let why = hook("t1.step3", 401).unwrap();
    assert!(
        why.starts_with("FERB stopped this command: this task has received 0 MB in downloads"),
        "{why}"
    );
    assert_eq!(json!(*told.lock().unwrap()), json!([["t1.step3", why]]));
    let rows = select(
        &rusqlite::Connection::open(&path).unwrap(),
        "SELECT tree_id, bytes FROM shell_downloads",
    );
    assert_eq!(json!(rows), json!([{"tree_id": "t1", "bytes": 1001}]));
}
