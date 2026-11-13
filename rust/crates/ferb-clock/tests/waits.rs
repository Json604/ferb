//! (d) `wait.for` against what the Python tool did (`rust/fixtures/clock/waits.json`): the same scripted events on
//! a stepped clock (TEST-508: each rest moves the clock at once), the same fakes for the task's commands and tabs,
//! and the same wake reason, timing, result, read sources and live line.
//! decisions: ORCH-631, ORCH-632, ORCH-633, ORCH-639

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::task::{Context, Poll};

use common::{fixture, same, tool_ctx, Diffs, YieldOnce};
use ferb_clock::py::{self, Time};
use ferb_clock::wait::{
    Browser, PageError, ProcHandle, ProcInfo, Shells, Snap, WaitEnv, WaitFor, WaitInput,
};
use ferb_clock::zone::Zone;
use ferb_core::errors::FerbError;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{Identity, ToolContext};
use serde_json::{json, Map, Value};

/// `RefCell` and `Cell` by name, on a mutex: the seams' futures are `Send`, so the fakes are `Sync`.
#[derive(Default)]
struct RefCell<T>(std::sync::Mutex<T>);

impl<T> RefCell<T> {
    fn new(v: T) -> Self {
        RefCell(std::sync::Mutex::new(v))
    }
    fn borrow(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
    fn borrow_mut(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct Cell<T>(std::sync::Mutex<T>);

impl<T: Copy> Cell<T> {
    fn new(v: T) -> Self {
        Cell(std::sync::Mutex::new(v))
    }
    fn get(&self) -> T {
        *self.0.lock().unwrap()
    }
    fn set(&self, v: T) {
        *self.0.lock().unwrap() = v;
    }
}

// ---------- the fakes (tests/test_wait_for.py's, plus scripted check-script runs) ----------

#[derive(Default)]
struct FakeShells {
    procs: RefCell<BTreeMap<String, ProcInfo>>,
    order: RefCell<Vec<String>>,
    out: RefCell<BTreeMap<String, String>>,
    told: RefCell<BTreeSet<String>>,
    scripts: Vec<Value>,
    runs: Cell<usize>,
    hosts: RefCell<Vec<String>>,
    released: RefCell<Vec<String>>,
    killed: RefCell<Vec<String>>,
    started: RefCell<Vec<Value>>,
}

impl FakeShells {
    fn add(&self, id: &str, command: &str, task: &str) {
        self.procs.borrow_mut().insert(
            id.into(),
            ProcInfo {
                id: id.into(),
                command: command.into(),
                running: true,
                exit_code: None,
                stopped: None,
                owner_task: task.into(),
            },
        );
        self.order.borrow_mut().push(id.into());
        self.out.borrow_mut().insert(id.into(), String::new());
    }

    fn finish(&self, id: &str, code: i64, stopped: Option<String>) {
        let mut p = self.procs.borrow_mut();
        let i = p.get_mut(id).unwrap();
        i.running = false;
        i.exit_code = Some(code);
        i.stopped = stopped;
    }

    fn chars(text: &str, from: usize) -> String {
        text.chars().skip(from).collect()
    }
}

fn ready<T: Send + 'static>(v: T) -> BoxFuture<'static, T> {
    Box::pin(std::future::ready(v))
}

impl Shells for FakeShells {
    fn handle(&self, id: &str, task: &str) -> ProcHandle {
        ProcHandle {
            id: id.into(),
            owner_task: task.into(),
        }
    }
    fn info(&self, h: &ProcHandle) -> Result<ProcInfo, FerbError> {
        match self.procs.borrow().get(&h.id) {
            Some(i) if i.owner_task == h.owner_task => Ok(i.clone()),
            _ => Err(FerbError::Refused(format!(
                "no process {} for this task",
                h.id
            ))),
        }
    }
    fn list(&self) -> Vec<ProcInfo> {
        let procs = self.procs.borrow();
        self.order
            .borrow()
            .iter()
            .filter_map(|id| procs.get(id).cloned())
            .collect()
    }
    fn told(&self, id: &str) -> bool {
        self.told.borrow().contains(id)
    }
    fn exit_told(&self, h: &ProcHandle) {
        self.told.borrow_mut().insert(h.id.clone());
    }
    fn read<'a>(
        &'a self,
        h: &'a ProcHandle,
        since: usize,
    ) -> BoxFuture<'a, (String, usize, usize)> {
        let text = self.out.borrow()[&h.id].clone();
        let n = text.chars().count();
        ready((Self::chars(&text, since), n, n))
    }
    fn tail<'a>(
        &'a self,
        h: &'a ProcHandle,
        limit: usize,
    ) -> BoxFuture<'a, (String, usize, usize)> {
        let text = self.out.borrow()[&h.id].clone();
        let n = text.chars().count();
        ready((Self::chars(&text, n.saturating_sub(limit)), n, n))
    }
    fn wait<'a>(&'a self, h: &'a ProcHandle, _timeout: f64) -> BoxFuture<'a, Option<i64>> {
        if h.id.starts_with("s-") {
            return ready(self.procs.borrow()[&h.id].exit_code);
        }
        Box::pin(std::future::poll_fn(move |_cx| {
            let p = self.procs.borrow();
            let i = &p[&h.id];
            if i.running {
                Poll::Pending
            } else {
                Poll::Ready(i.exit_code)
            }
        }))
    }
    fn start<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        task: &'a str,
        _folders: &'a [String],
    ) -> BoxFuture<'a, Result<ProcHandle, FerbError>> {
        let run = self.scripts[self.runs.get().min(self.scripts.len() - 1)].clone();
        self.runs.set(self.runs.get() + 1);
        let id = format!("s-{}", self.runs.get());
        self.started.borrow_mut().push(json!([command, cwd, task]));
        let code = run["code"].as_i64();
        self.procs.borrow_mut().insert(
            id.clone(),
            ProcInfo {
                id: id.clone(),
                command: command.into(),
                running: code.is_none(),
                exit_code: code,
                stopped: run["stopped"].as_str().map(str::to_string),
                owner_task: task.into(),
            },
        );
        self.out
            .borrow_mut()
            .insert(id.clone(), run["output"].as_str().unwrap_or("").into());
        *self.hosts.borrow_mut() = run["hosts"]
            .as_array()
            .map(|a| a.iter().map(|h| h.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        ready(Ok(ProcHandle {
            id,
            owner_task: task.into(),
        }))
    }
    fn kill<'a>(&'a self, h: &'a ProcHandle) -> BoxFuture<'a, ()> {
        self.killed.borrow_mut().push(h.id.clone());
        if let Some(p) = self.procs.borrow_mut().get_mut(&h.id) {
            p.running = false;
        }
        ready(())
    }
    fn release<'a>(&'a self, h: &'a ProcHandle) -> BoxFuture<'a, ()> {
        self.released.borrow_mut().push(h.id.clone());
        self.procs.borrow_mut().remove(&h.id);
        ready(())
    }
    fn network(&self, _h: &ProcHandle) -> Vec<String> {
        self.hosts.borrow().clone()
    }
    fn working_directory(&self, cwd: Option<&str>, ctx: &ToolContext) -> Result<String, FerbError> {
        match cwd {
            Some(c) if c.starts_with("/forbidden") => Err(FerbError::Refused(format!(
                "{c} is outside this task's folders"
            ))),
            Some(c) => Ok(c.into()),
            None => Ok(ctx.workspace.clone()),
        }
    }
}

#[derive(Default)]
struct FakeBrowser {
    url: RefCell<String>,
    pages: Vec<String>,
    reads: Cell<usize>,
    reloads: Cell<usize>,
    downloads: RefCell<Vec<Vec<Value>>>,
    fail_reads: Vec<usize>,
    no_downloads: bool,
}

impl Browser for FakeBrowser {
    fn owned_by(&self, tab: &str, task: &str) -> Result<String, PageError> {
        if tab != "t1" || task != "t1" {
            return Err(PageError::Refused(format!(
                "no tab {tab} open for this task; its open tabs: t1"
            )));
        }
        Ok(tab.into())
    }
    fn tab_url(&self, _tab: &str) -> Option<String> {
        Some(self.url.borrow().clone())
    }
    fn snapshot<'a>(&'a self, _r: &'a str) -> BoxFuture<'a, Result<Snap, PageError>> {
        self.reads.set(self.reads.get() + 1);
        let n = self.reads.get();
        if self.fail_reads.contains(&n) {
            return ready(Err(PageError::Failed {
                kind: "TimeoutError".into(),
                message: format!("read {n} timed out"),
            }));
        }
        let text = self.pages[n.min(self.pages.len()) - 1].clone();
        ready(Ok(Snap {
            url: self.url.borrow().clone(),
            title: Some("Product\u{7} page".into()),
            text: Some(text),
        }))
    }
    fn navigate<'a>(&'a self, _r: &'a str, _url: &'a str) -> BoxFuture<'a, Result<(), PageError>> {
        self.reloads.set(self.reloads.get() + 1);
        ready(Ok(()))
    }
    fn downloads_for<'a>(
        &'a self,
        _r: &'a str,
    ) -> Option<BoxFuture<'a, Result<Vec<Value>, PageError>>> {
        if self.no_downloads {
            return None;
        }
        let mut d = self.downloads.borrow_mut();
        let next = if d.is_empty() { vec![] } else { d.remove(0) };
        Some(ready(Ok(next)))
    }
}

/// The stepped clock and everything else the wait is given.
struct Env {
    wall: Cell<Time>,
    mono: Cell<f64>,
    rests: Cell<usize>,
    events: RefCell<Vec<(Value, bool)>>,
    shells: FakeShells,
    browser: Option<FakeBrowser>,
    lines: RefCell<Vec<Value>>,
    steer: Cell<i64>,
    ending: Option<String>,
    ended: RefCell<Vec<Value>>,
    failed: RefCell<Option<Map<String, Value>>>,
    limit: Option<Time>,
    work: PathBuf,
    home: PathBuf,
    zone: Zone,
}

impl Env {
    fn apply_events(&self) {
        let mut events = self.events.borrow_mut();
        for (e, done) in events.iter_mut() {
            if *done || self.mono.get() < e["at"].as_f64().unwrap() {
                continue;
            }
            *done = true;
            match e["do"].as_str().unwrap() {
                "finish" => self.shells.finish(
                    e["id"].as_str().unwrap(),
                    e["code"].as_i64().unwrap(),
                    e["stopped"].as_str().map(str::to_string),
                ),
                "output" => {
                    self.shells.out.borrow_mut().insert(
                        e["id"].as_str().unwrap().into(),
                        e["text"].as_str().unwrap().into(),
                    );
                }
                "steer" => self.steer.set(self.steer.get() + 1),
                "url" => {
                    *self.browser.as_ref().unwrap().url.borrow_mut() =
                        e["url"].as_str().unwrap().into()
                }
                kind @ ("write" | "append") => {
                    let p = self.work.join(e["path"].as_str().unwrap());
                    let mut bytes = if kind == "append" {
                        std::fs::read(&p).unwrap_or_default()
                    } else {
                        vec![]
                    };
                    bytes.extend(std::iter::repeat_n(
                        b'x',
                        e["bytes"].as_u64().unwrap() as usize,
                    ));
                    std::fs::write(&p, bytes).unwrap();
                    set_mtime(&p, e["mtime"].as_i64().unwrap());
                }
                "delete" => {
                    std::fs::remove_file(self.work.join(e["path"].as_str().unwrap())).unwrap()
                }
                other => panic!("unknown event {other}"),
            }
        }
    }
}

fn set_mtime(p: &Path, secs: i64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
    std::fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

impl WaitEnv for Env {
    fn now(&self) -> Time {
        self.wall.get()
    }
    fn monotonic(&self) -> f64 {
        self.mono.get()
    }
    fn pause(&self, seconds: f64) -> BoxFuture<'_, ()> {
        self.rests.set(self.rests.get() + 1);
        let us = py::timedelta_us(seconds, py::US_PER_SECOND).unwrap();
        self.wall.set(py::plus_us(&self.wall.get(), us).unwrap());
        self.mono.set(self.mono.get() + seconds);
        self.apply_events();
        Box::pin(YieldOnce(false))
    }
    fn limit(&self, _task: &str) -> Option<Time> {
        self.limit
    }
    fn shells(&self) -> Option<&dyn Shells> {
        Some(&self.shells)
    }
    fn browser(&self, _identity: Identity) -> Option<&dyn Browser> {
        self.browser.as_ref().map(|b| b as &dyn Browser)
    }
    fn status(&self, _worker: &str, _task: &str, until: Option<&Time>, line: Option<&str>) {
        self.lines
            .borrow_mut()
            .push(json!([until.map(py::iso), line]));
    }
    fn steered(&self, _task: &str) -> i64 {
        self.steer.get()
    }
    fn ending(&self) -> Option<String> {
        self.ending.clone()
    }
    fn ended(&self, task: &str, data: Value) {
        self.ended.borrow_mut().push(json!([task, data]));
    }
    fn failed_act(&self, worker: &str) -> Option<Map<String, Value>> {
        if worker == "w1" {
            self.failed.borrow_mut().take()
        } else {
            None
        }
    }
    fn zone(&self) -> Zone {
        self.zone
    }
    fn home(&self) -> PathBuf {
        self.home.clone()
    }
    fn host(&self, url: &str) -> String {
        let rest = url.split_once("://").map_or("", |(_, r)| r);
        let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = hostport.rsplit('@').next().unwrap_or("");
        host.split(':').next().unwrap_or("").to_lowercase()
    }
}

/// Polls a future to its end with a waker that does nothing (the fakes never block), at most `limit` times.
fn drive<F: std::future::Future>(f: F, limit: usize) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    for _ in 0..limit {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return Some(v);
        }
    }
    None
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn placeholders(v: &Value, work: &Path, home: &Path) -> Value {
    let text = v
        .to_string()
        .replace("<WORK>", &work.to_string_lossy())
        .replace("<HOME>", &home.to_string_lossy());
    serde_json::from_str(&text).unwrap()
}

#[test]
fn waits_wake_as_python_did() {
    let f = fixture("waits.json");
    let start = py::parse(f["start"].as_str().unwrap()).unwrap();
    let zone = Zone::named("Asia/Kolkata").unwrap();
    let mut d = Diffs::default();
    for sc in f["scenarios"].as_array().unwrap() {
        let Some(runs) = sc.get("runs") else { continue };
        let name = sc["name"].as_str().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let work = tmp.path().join("work");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        for dir in strs(&sc["dirs"]) {
            std::fs::create_dir_all(work.join(dir)).unwrap();
        }
        for file in sc["files"].as_array().into_iter().flatten() {
            let p = work.join(file[0].as_str().unwrap());
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![b'x'; file[1].as_u64().unwrap() as usize]).unwrap();
            set_mtime(&p, file[2].as_i64().unwrap());
        }
        let shells = FakeShells {
            scripts: sc["scripts"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![json!({"code": 0})]),
            ..FakeShells::default()
        };
        for p in sc["procs"].as_array().into_iter().flatten() {
            shells.add(
                p[0].as_str().unwrap(),
                p[1].as_str().unwrap(),
                p.get(2).and_then(Value::as_str).unwrap_or("t1"),
            );
        }
        for (id, text) in sc["out"].as_object().into_iter().flatten() {
            shells
                .out
                .borrow_mut()
                .insert(id.clone(), text.as_str().unwrap().into());
        }
        for id in strs(&sc["told"]) {
            shells.told.borrow_mut().insert(id);
        }
        for p in sc["finished"].as_array().into_iter().flatten() {
            shells.finish(p[0].as_str().unwrap(), p[1].as_i64().unwrap(), None);
        }
        let browser = FakeBrowser {
            url: RefCell::new(
                sc["url"]
                    .as_str()
                    .unwrap_or("https://www.example.in/dp/B0X")
                    .into(),
            ),
            pages: strs(&sc["pages"]),
            downloads: RefCell::new(
                sc["downloads"]
                    .as_array()
                    .map(|a| a.iter().map(|x| x.as_array().unwrap().clone()).collect())
                    .unwrap_or_default(),
            ),
            fail_reads: sc["fail_reads"]
                .as_array()
                .map(|a| a.iter().map(|x| x.as_u64().unwrap() as usize).collect())
                .unwrap_or_default(),
            no_downloads: sc["no_downloads"].as_bool().unwrap_or(false),
            ..FakeBrowser::default()
        };
        let limit = match sc.get("task_limit_min") {
            None => Some(240),
            Some(Value::Null) => None,
            Some(v) => v.as_i64(),
        }
        .map(|m| py::plus_us(&start, m * py::US_PER_MINUTE).unwrap());
        let env = Env {
            wall: Cell::new(start),
            mono: Cell::new(0.0),
            rests: Cell::new(0),
            events: RefCell::new(
                sc["events"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| (e, false))
                    .collect(),
            ),
            shells,
            browser: Some(browser),
            lines: RefCell::new(vec![]),
            steer: Cell::new(0),
            ending: None,
            ended: RefCell::new(vec![]),
            failed: RefCell::new(sc["failed"].as_object().cloned()),
            limit,
            work: work.clone(),
            home: home.clone(),
            zone,
        };
        let identity = if sc["identity"] == "owner" {
            Identity::Owner
        } else {
            Identity::Ferb
        };
        let scopes: Vec<String> = sc["scopes"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_else(|| {
                [
                    "time",
                    "shell",
                    "fs:read",
                    "identity:browse",
                    "browser:read",
                ]
                .map(String::from)
                .to_vec()
            });
        let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
        let ctx = tool_ctx(identity, &scope_refs, &work.to_string_lossy(), None);
        let tool = WaitFor::new(&env);
        for (i, want) in runs.as_array().unwrap().iter().enumerate() {
            let want = placeholders(want, &work, &home);
            let what = format!("{name} run {i}");
            let input = WaitInput::from_json(&want["args"]).expect("arguments Python accepted");
            let res = drive(tool.run(&input, &ctx), 1_000_000).expect("the wait ends");
            let got = json!({"content": Value::Object(res.content), "is_error": res.is_error,
                             "read_sources": res.read_sources.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()});
            d.check(&format!("{what} result"), &want["result"], &got);
            d.check(
                &format!("{what} lines"),
                &want["lines"],
                &json!(env.lines.borrow_mut().split_off(0)),
            );
            for (k, v) in [
                ("rests", json!(env.rests.get())),
                ("mono", json!(env.mono.get())),
                ("wall", json!(py::iso(&env.wall.get()))),
                ("reads", json!(env.browser.as_ref().unwrap().reads.get())),
                (
                    "reloads",
                    json!(env.browser.as_ref().unwrap().reloads.get()),
                ),
                ("released", json!(*env.shells.released.borrow())),
                ("killed", json!(*env.shells.killed.borrow())),
                ("started", json!(*env.shells.started.borrow())),
                ("told", json!(*env.shells.told.borrow())),
            ] {
                d.check(&format!("{what} {k}"), &want[k], &v);
            }
        }
    }
    d.finish("waits");
}

/// decision: ORCH-633. A stop drops the wait: its live line is cleared; only a restart journals `wait.ended`.
#[test]
fn a_stopped_wait_clears_its_line_and_a_restart_leaves_a_fact() {
    let f = fixture("waits.json");
    let start = py::parse(f["start"].as_str().unwrap()).unwrap();
    for ending in [None, Some("restart")] {
        let want = f["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == format!("cancelled_{}", ending.unwrap_or("None")))
            .unwrap()
            .clone();
        let shells = FakeShells::default();
        shells.add("p-1", "codex exec 'fix the tests'", "t1");
        let env = Env {
            wall: Cell::new(start),
            mono: Cell::new(0.0),
            rests: Cell::new(0),
            events: RefCell::new(vec![]),
            shells,
            browser: None,
            lines: RefCell::new(vec![]),
            steer: Cell::new(0),
            ending: ending.map(str::to_string),
            ended: RefCell::new(vec![]),
            failed: RefCell::new(None),
            limit: None,
            work: PathBuf::from("/w"),
            home: PathBuf::from("/h"),
            zone: Zone::named("Asia/Kolkata").unwrap(),
        };
        let ctx = tool_ctx(Identity::Ferb, &["time", "shell"], "/w", None);
        let input =
            WaitInput::from_json(&json!({"any_of": [{"kind": "process", "id": "p-1"}]})).unwrap();
        let tool = WaitFor::new(&env);
        assert!(
            drive(tool.run(&input, &ctx), 20).is_none(),
            "still waiting when stopped"
        );
        assert!(
            same(&want["lines"], &json!(*env.lines.borrow())),
            "{}",
            json!(*env.lines.borrow())
        );
        assert!(
            same(&want["ended"], &json!(*env.ended.borrow())),
            "{}",
            json!(*env.ended.borrow())
        );
    }
}

/// The model's own checks on its arguments, and what the gateway is told a wait does (`describe`, `file_paths`).
#[test]
fn the_input_checks_and_what_the_gateway_sees_match_python() {
    let f = fixture("waits.json");
    let mut d = Diffs::default();
    for case in f["checks"].as_array().unwrap() {
        let what = format!("check {}", case["args"]);
        match WaitInput::from_json(&case["args"]) {
            Err(e) => {
                let want = case["messages"][0]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("Value error, ");
                if want.starts_with("match isn't a valid regular expression: ") {
                    // fancy-regex's words after the prefix, not Python's re (listed in the journal)
                    d.check(
                        &what,
                        &json!(true),
                        &json!(e.starts_with("match isn't a valid regular expression: ")),
                    );
                } else {
                    d.check(&what, &json!(want), &json!(e));
                }
            }
            Ok(input) => {
                let spec = input.describe();
                let got = json!({"args": case["args"], "kind": spec.kind, "path": spec.target.path,
                                 "content": spec.content,
                                 "file_paths": input.file_paths().iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()});
                d.check(&what, case, &got);
            }
        }
    }
    d.check(
        "description",
        &f["description"],
        &json!(ferb_clock::wait::DESCRIPTION),
    );
    d.finish("wait checks");
}

/// Fixed against Python: a file first seen exactly at monotonic 0.0 never settled there (`settled_since or
/// monotonic()` reads 0.0 as unset); here it settles after `settle_s` like any other.
#[test]
fn a_file_seen_at_monotonic_zero_still_settles() {
    let tmp = tempfile::TempDir::new().unwrap();
    let work = tmp.path().to_path_buf();
    std::fs::write(work.join("done.csv"), b"a,b").unwrap();
    let env = Env {
        wall: Cell::new(py::parse("2026-11-12T16:00:00+00:00").unwrap()),
        mono: Cell::new(0.0),
        rests: Cell::new(0),
        events: RefCell::new(vec![]),
        shells: FakeShells::default(),
        browser: None,
        lines: RefCell::new(vec![]),
        steer: Cell::new(0),
        ending: None,
        ended: RefCell::new(vec![]),
        failed: RefCell::new(None),
        limit: None,
        work: work.clone(),
        home: work.clone(),
        zone: Zone::utc(),
    };
    let ctx = tool_ctx(
        Identity::Ferb,
        &["time", "fs:read"],
        &work.to_string_lossy(),
        None,
    );
    let input = WaitInput::from_json(
        &json!({"any_of": [{"kind": "file", "path": "done.csv", "state": "settled", "settle_s": 4}], "limit_minutes": 1}),
    )
    .unwrap();
    let res = drive(WaitFor::new(&env).run(&input, &ctx), 100_000).unwrap();
    assert_eq!(res.content["woke"], "file", "{:?}", res.content);
    assert_eq!(res.content["evidence"]["unchanged_s"], 4);
}
