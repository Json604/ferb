//! `ferb/daemon/areas/desktop.py` against `rust/fixtures/daemon-b/desktop.json` (made by `desktop.py` beside it):
//! each case's steps run on the Rust functions over fakes built from the recorded state, comparing every answer,
//! every call the fakes received (in order), the journal rows, the published events, the device profile's files and
//! the Automation state left behind, byte for byte (`json.dumps`).

mod b_support;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use b_support::{fixture, Home};
use ferb_core::interfaces::BoxFuture;
use ferb_daemon::areas_b::desktop::{
    self as dk, Automation, CaptureError, DaemonSources, MacDriver, NativeFront, Probe, Rect,
    Seams, ShellProc, Spans, TaskRef, WindowImage,
};
use ferb_daemon::areas_b::environment::{ReleaseError, TabDriver};
use ferb_device::profile::AppInfo;
use ferb_sense::front_window::Window;
use ferb_sense::watchers::browser_tabs::ScriptedWindow;
use ferb_sense::Dict;
use serde_json::{json, Map, Value};

type Log = Arc<Mutex<Vec<Value>>>;

fn s(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_default()
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

/// A Python exception as `f"{type(e).__name__}: {e}"` writes it (a KeyError's text is its key's repr).
fn py_err(spec: &str) -> String {
    let (kind, text) = spec.split_once(": ").unwrap_or((spec, ""));
    if kind == "KeyError" {
        format!("{kind}: '{text}'")
    } else {
        format!("{kind}: {text}")
    }
}

fn kind_of(spec: &str) -> String {
    spec.split_once(": ").map_or(spec, |(k, _)| k).to_owned()
}

fn rect(v: &Value) -> Rect {
    let f = |i: usize| v[i].as_f64().unwrap();
    (f(0), f(1), f(2), f(3))
}

fn rect_json(r: Rect) -> Value {
    json!([r.0, r.1, r.2, r.3])
}

fn window(v: &Value) -> Window {
    Window {
        id: v["id"].as_i64().unwrap(),
        pid: v["pid"].as_i64().unwrap(),
        bounds: rect(&v["bounds"]),
        title: s(&v["title"]),
        layer: 0,
        alpha: 1.0,
    }
}

fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

struct FakeSpans {
    log: Log,
    n: Mutex<usize>,
}

impl Spans for FakeSpans {
    fn start(&self, kind: &str, task_id: &str, parent_id: Option<&str>, input: Value) -> String {
        let mut n = self.n.lock().unwrap();
        *n += 1;
        self.log
            .lock()
            .unwrap()
            .push(json!(["spans.start", kind, task_id, parent_id, input]));
        format!("span-{n}")
    }
    fn finish(&self, span_id: &str, output: Value) {
        self.log
            .lock()
            .unwrap()
            .push(json!(["spans.finish", span_id, output]));
    }
}

struct Driver {
    st: Value,
    name: &'static str,
    log: Log,
}

impl TabDriver for Driver {
    fn task_tabs(&self, _: &str) -> Vec<Map<String, Value>> {
        vec![]
    }
    fn urls_of(&self, _: &str) -> Vec<String> {
        vec![]
    }
    fn keep_tabs_open(&self, _: &str) {}
    fn rest<'a>(&'a self, _: &'a str, _: bool) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn release<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<Vec<Value>, ReleaseError>> {
        Box::pin(async move {
            self.log
                .lock()
                .unwrap()
                .push(json!([format!("{}.release", self.name), task_id]));
            if let Some(e) = self.st.get("raise") {
                return Err(ReleaseError {
                    error: py_err(&s(e)),
                    tabs: self.st.get("tabs").cloned(),
                });
            }
            Ok(list(&self.st["release"]))
        })
    }
    fn has_wake(&self) -> bool {
        false
    }
}

struct Mac {
    st: Value,
    world: Value,
    log: Log,
}

impl Mac {
    fn push(&self, v: Value) {
        self.log.lock().unwrap().push(v);
    }
}

impl MacDriver for Mac {
    fn page_secrets<'a>(
        &'a self,
        pid: i64,
        bounds: Rect,
        title: &'a str,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            self.push(json!(["mac.page_secrets", pid, rect_json(bounds), title]));
            if let Some(e) = self.st.get("page_secrets_raise") {
                return Err(kind_of(&s(e)));
            }
            Ok(self.st.get("page_secrets").cloned().unwrap_or(Value::Null))
        })
    }
    fn place_on_web_area<'a>(
        &'a self,
        r: Rect,
        v: Rect,
        pid: i64,
        b: Rect,
        title: &'a str,
    ) -> BoxFuture<'a, Result<Option<Rect>, String>> {
        Box::pin(async move {
            self.push(json!([
                "mac.place_on_web_area",
                rect_json(r),
                rect_json(v),
                pid,
                rect_json(b),
                title
            ]));
            Ok(self.st.get("place").map(rect))
        })
    }
    fn snapshot_images(&self) -> bool {
        self.st["snapshot_images"].as_bool().unwrap_or(true)
    }
    fn window_image<'a>(
        &'a self,
        target: &'a str,
        w: &'a str,
    ) -> BoxFuture<'a, Result<WindowImage, CaptureError>> {
        Box::pin(async move {
            self.push(json!(["mac.window_image", target, w]));
            if self.st["image_pending"].as_bool().unwrap_or(false) {
                std::future::pending::<()>().await;
            }
            if let Some(e) = self.st.get("image_raise") {
                let e = s(e);
                let (kind, text) = e.split_once(": ").unwrap_or((&e, ""));
                return Err(match kind {
                    "NoTarget" => CaptureError::NoTarget(text.into()),
                    "Withheld" => CaptureError::Withheld(text.into()),
                    "Refused" => CaptureError::Refused(text.into()),
                    other => CaptureError::Failed(other.into()),
                });
            }
            let jpeg = self.st.get("jpeg").map(s).unwrap_or_else(|| "jpeg".into());
            Ok((jpeg.into_bytes(), s(&self.st["masking"]), None))
        })
    }
    fn normal_windows(&self) -> Result<Vec<Window>, String> {
        self.push(json!(["screenshot.normal_windows"]));
        if let Some(e) = self.world.get("windows_raise") {
            return Err(kind_of(&s(e)));
        }
        Ok(list(&self.world["windows"]).iter().map(window).collect())
    }
    fn window(&self, id: i64) -> Result<Option<Window>, String> {
        self.push(json!(["screenshot.window", id]));
        Ok(self.world.get("window_now").map(window))
    }
    fn has_launched(&self) -> bool {
        self.st.get("launched").is_some()
    }
    fn release_launched<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Value>, String>> {
        Box::pin(async move {
            self.push(json!(["launched.release", task_id]));
            let l = &self.st["launched"];
            if let Some(e) = l.get("raise") {
                return Err(py_err(&s(e)));
            }
            Ok(list(&l["release"]))
        })
    }
    fn app_window<'a>(
        &'a self,
        _: &'a str,
        _: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Result<Option<Value>, String>> {
        Box::pin(async { Ok(None) })
    }
}

struct FakeProbe(Mutex<Option<String>>);

impl Probe for FakeProbe {
    fn done(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }
    fn granted(&self) -> bool {
        self.0.lock().unwrap().as_deref() == Some("granted")
    }
}

struct Fake {
    st: Value,
    log: Log,
    probes: Mutex<HashMap<String, Vec<String>>>,
    held: Mutex<HashMap<String, Arc<FakeProbe>>>,
}

impl Fake {
    fn push(&self, v: Value) {
        self.log.lock().unwrap().push(v);
    }
    fn ender<'a>(&'a self, name: &'static str, task_id: &'a str) -> BoxFuture<'a, Vec<Value>> {
        Box::pin(async move {
            self.push(json!([format!("{name}.end_task"), task_id]));
            list(&self.st[name]["end"][task_id])
        })
    }
    fn owners(&self, name: &str) -> Vec<Option<String>> {
        list(&self.st[name]["owners"])
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect()
    }
    fn driver(&self, key: &str, name: &'static str) -> Option<Arc<dyn TabDriver>> {
        let st = self.st.get(key).filter(|v| !v.is_null())?;
        Some(Arc::new(Driver {
            st: st.clone(),
            name,
            log: self.log.clone(),
        }))
    }
}

impl Seams for Fake {
    fn task(&self, task_id: &str) -> Option<TaskRef> {
        let t = self.st["tasks"].get(task_id)?;
        Some(TaskRef {
            span_id: t["span_id"].as_str().map(str::to_owned),
            tree_id: t["tree_id"].as_str().map(str::to_owned),
        })
    }
    fn spans(&self) -> Option<Arc<dyn Spans>> {
        if self.st["spans"] == Value::Bool(false) {
            return None;
        }
        Some(Arc::new(FakeSpans {
            log: self.log.clone(),
            n: Mutex::new(0),
        }))
    }
    fn mac(&self) -> Option<Arc<dyn MacDriver>> {
        let st = self.st.get("mac").filter(|v| !v.is_null())?;
        Some(Arc::new(Mac {
            st: st.clone(),
            world: self.st.clone(),
            log: self.log.clone(),
        }))
    }
    fn hearing_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Vec<Value>> {
        self.ender("hearing", task_id)
    }
    fn recordings_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Vec<Value>> {
        self.ender("recordings", task_id)
    }
    fn listening_owners(&self) -> Vec<Option<String>> {
        self.owners("hearing")
    }
    fn recording_owners(&self) -> Vec<Option<String>> {
        self.owners("recordings")
    }
    fn identity_browser(&self) -> Option<Arc<dyn TabDriver>> {
        self.driver("identity_browser", "identity_browser")
    }
    fn router(&self) -> Option<Arc<dyn TabDriver>> {
        self.driver("router", "router")
    }
    fn forget_downloads(&self, task_id: &str) {
        if self.st["downloads"].as_bool().unwrap_or(false) {
            self.push(json!(["downloads.forget_task", task_id]));
        }
    }
    fn has_shells(&self) -> bool {
        self.st.get("shells").is_some_and(|v| !v.is_null())
    }
    fn shells_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.push(json!(["shells.end_task", task_id]));
            match self.st["shells"].get("end_raise") {
                Some(e) => Err(py_err(&s(e))),
                None => Ok(()),
            }
        })
    }
    fn shells_stop_running<'a>(
        &'a self,
        mine: &'a (dyn Fn(Option<&str>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, Result<usize, String>> {
        Box::pin(async move {
            let matched: Vec<Value> = list(&self.st["shells"]["running_owners"])
                .into_iter()
                .filter(|o| mine(o.as_str()))
                .collect();
            self.push(json!(["shells.stop_running", matched]));
            match &self.st["shells"]["stop"] {
                Value::String(e) => Err(py_err(e)),
                v => Ok(v.as_u64().unwrap_or(0) as usize),
            }
        })
    }
    fn shell_processes(&self) -> Vec<ShellProc> {
        list(&self.st["shells"]["procs"])
            .iter()
            .map(|p| ShellProc {
                id: s(&p["id"]),
                owner_task: p["owner_task"].as_str().map(str::to_owned),
                command: s(&p["command"]),
                long_lived: p["long_lived"].as_bool().unwrap(),
                running: p["running"].as_bool().unwrap(),
                started: p["started"].as_f64().unwrap(),
            })
            .collect()
    }
    fn has_schedules(&self) -> bool {
        self.st.get("schedules").is_some_and(|v| !v.is_null())
    }
    fn schedule_state(&self, id: &str) -> Option<String> {
        self.st["schedules"].get(id).map(s)
    }
    fn schedule_change(&self, id: &str, action: &str, with_task: &str) {
        self.push(json!(["schedule_change", id, action, with_task]));
    }
    fn journal_latest_kind(&self, kinds: &[&str], key: &str, value: &str) -> Option<String> {
        self.push(json!(["journal.latest_kind", kinds, key, value]));
        self.st["latest_kind"].get(value).map(s)
    }
    fn journal_of_kind(&self, kind: &str, key: &str, value: &str) -> Vec<Value> {
        self.push(json!(["journal.of_kind", kind, key, value]));
        list(&self.st["of_kind"][value])
    }
    fn revoke_tree(&self, tree: &str) {
        self.push(json!(["tokens.revoke_tree", tree]));
    }
    fn disturbance_finish<'a>(&'a self, tree: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.push(json!(["disturbance.finish", tree]));
            match self.st.get("disturbance_raise") {
                Some(e) => Err(py_err(&s(e))),
                None => Ok(()),
            }
        })
    }
    fn changes_forget(&self, tree: &str) -> Option<Vec<String>> {
        let c = self.st.get("changes").filter(|v| !v.is_null())?;
        self.push(json!(["changes.forget", tree]));
        Some(list(&c["stuck"]).iter().map(s).collect())
    }
    fn has_uploads(&self) -> bool {
        self.st["uploads"].as_bool().unwrap_or(false)
    }
    fn clean_uploads<'a>(&'a self, tree: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.push(json!(["clean_uploads", "forget_tree", tree, {"task_id": tree}]));
        })
    }
    fn automation_check(&self, bundle_id: &str) -> Arc<dyn Probe> {
        self.push(json!(["automation_check", bundle_id, false]));
        let answer = {
            let mut p = self.probes.lock().unwrap();
            let q = p.entry(bundle_id.to_owned()).or_default();
            if q.is_empty() {
                "pending".to_owned()
            } else {
                q.remove(0)
            }
        };
        let probe = Arc::new(FakeProbe(Mutex::new(
            (answer != "pending").then_some(answer),
        )));
        self.held
            .lock()
            .unwrap()
            .insert(bundle_id.to_owned(), probe.clone());
        probe
    }
    fn applescript_windows(&self, bundle_id: &str) -> Result<Option<Vec<ScriptedWindow>>, String> {
        self.push(json!(["applescript_windows", bundle_id]));
        match self.st["scripted"].get(bundle_id) {
            None => Ok(None),
            Some(Value::String(_)) => Err("OSError".into()),
            Some(v) => Ok(Some(
                list(v)
                    .iter()
                    .map(|x| ScriptedWindow {
                        bounds: (!x["bounds"].is_null()).then(|| rect(&x["bounds"])),
                        name: s(&x["name"]),
                        url: s(&x["url"]),
                        title: s(&x["title"]),
                    })
                    .collect(),
            )),
        }
    }
    fn permission(&self, name: &str) -> Result<String, String> {
        match self.st["permissions"].get(name).map(s) {
            Some(v) if v == "raise" => Err("RuntimeError".into()),
            Some(v) => Ok(v),
            None => Ok("granted".into()),
        }
    }
    fn running_apps(&self) -> Result<Vec<(String, String, i64)>, String> {
        match &self.st["running"] {
            Value::String(_) => Err("RuntimeError".into()),
            v => Ok(list(v)
                .iter()
                .map(|a| (s(&a[0]), s(&a[1]), a[2].as_i64().unwrap()))
                .collect()),
        }
    }
}

fn raised(e: impl Into<String>) -> Value {
    json!({"raised": e.into()})
}

fn run_case(c: &Value) -> Vec<String> {
    let home = Home::new(&s(&c["config"]));
    for (rel, text) in c["start_files"].as_object().into_iter().flatten() {
        let p = home.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, s(text)).unwrap();
    }
    *home.clock.mono.lock().unwrap() = 1000.0;
    let st = c["state"].clone();
    let probes = st["probes"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), list(v).iter().map(s).collect()))
        .collect();
    let f = Fake {
        st: st.clone(),
        log: Arc::new(Mutex::new(vec![])),
        probes: Mutex::new(probes),
        held: Mutex::new(HashMap::new()),
    };
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE spans (task_id TEXT, kind TEXT, started_at TEXT, output_json TEXT)",
    )
    .unwrap();
    for r in list(&st["spans_rows"]) {
        conn.execute(
            "INSERT INTO spans VALUES (?, ?, ?, ?)",
            rusqlite::params![s(&r[0]), s(&r[1]), s(&r[2]), r[3].as_str()],
        )
        .unwrap();
    }
    let auto = Automation::default();
    let host = &home.host;
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut outs: Vec<Value> = Vec::new();
    for step in list(&c["steps"]) {
        let a = |i: usize| step[i].clone();
        let out: Value = match step[0].as_str().unwrap() {
            "mono" => {
                *home.clock.mono.lock().unwrap() += a(1).as_f64().unwrap();
                Value::Null
            }
            "resolve" => {
                let p = f.held.lock().unwrap().get(&s(&a(1))).cloned().unwrap();
                *p.0.lock().unwrap() = Some(s(&a(2)));
                Value::Null
            }
            "_record_disturbance" => {
                dk::record_disturbance(
                    host,
                    &f,
                    &s(&a(1)),
                    &s(&a(2)),
                    a(3).as_object().cloned().unwrap(),
                );
                Value::Null
            }
            "_task_end" => json!(rt.block_on(dk::task_end(&f, &s(&a(1))))),
            "_task_paused" => {
                rt.block_on(dk::task_paused(host, &f, a(1).as_object().unwrap()))
                    .unwrap();
                Value::Null
            }
            "_task_resumed" => {
                rt.block_on(dk::task_resumed(&f, a(1).as_object().unwrap()));
                Value::Null
            }
            "_tree_finished" => match rt.block_on(dk::tree_finished(host, &f, &s(&a(1)))) {
                Ok(()) => Value::Null,
                Err(e) => raised(e),
            },
            "_owner_used_app" => {
                let app = a(2);
                json!(dk::owner_used_app(
                    &f,
                    &conn,
                    &s(&a(1)),
                    app["pid"].as_i64().unwrap(),
                    &s(&app["at"])
                )
                .unwrap())
            }
            "_running_processes" => json!(dk::running_processes(host, &f)),
            "_shell_process" => json!(dk::shell_process(&f, &s(&a(1)))),
            "_native_page_secrets" => json!(rt
                .block_on(dk::native_page_secrets(
                    host,
                    &f,
                    a(1).as_i64().unwrap(),
                    rect(&a(2)),
                    &s(&a(3))
                ))
                .unwrap()),
            "_native_place" => {
                let r = rt
                    .block_on(dk::native_place(
                        &f,
                        rect(&a(1)),
                        rect(&a(2)),
                        a(3).as_i64().unwrap(),
                        rect(&a(4)),
                        &s(&a(5)),
                    ))
                    .unwrap();
                r.map_or(Value::Null, rect_json)
            }
            "_native_window_capture" => {
                let (img, why) = rt
                    .block_on(dk::native_window_capture(
                        host,
                        &f,
                        a(1).as_i64().unwrap(),
                        rect(&a(2)),
                        &s(&a(3)),
                    ))
                    .unwrap();
                json!([img.map(|b| String::from_utf8(b).unwrap()), why])
            }
            "_automation_granted" => json!(auto.granted(host, &f, &s(&a(1)))),
            "_front_tab" => {
                let after = step.get(3).and_then(Value::as_bool).unwrap_or(false);
                match auto.front_tab(host, &f, &s(&a(1)), &window(&a(2)), after) {
                    Ok(t) => json!(t.map(|(u, t)| [u, t])),
                    Err(e) => raised(format!(
                        "{}: {}",
                        if e.ambiguous {
                            "Ambiguous"
                        } else {
                            "LookupError"
                        },
                        e.message
                    )),
                }
            }
            "_app_changed" => {
                // the spawn is the caller's (the composition runs it off the loop)
                f.push(json!(["spawn", "device profile update"]));
                let info = a(2);
                let info = (!info.is_null()).then(|| {
                    AppInfo::new(
                        &s(&info["bundle_id"]),
                        &s(&info["name"]),
                        &s(&info["version"]),
                        &s(&info["path"]),
                        info["scriptable"].as_bool().unwrap(),
                    )
                });
                dk::app_changed(host, &s(&a(1)), info.as_ref(), &s(&a(3))).unwrap();
                Value::Null
            }
            "_device_profile" => match dk::device_profile(host).unwrap() {
                Some(p) => serde_json::from_str(&p.to_json()).unwrap(),
                None => Value::Null,
            },
            "_installed_apps" => json!(dk::installed_apps(host).unwrap()),
            "_permissions" => Value::Object(dk::permissions(&f)),
            "_running_apps" => json!(dk::running_apps(&f)),
            m => panic!("unknown step {m}"),
        };
        outs.push(out);
    }
    let mut files = Map::new();
    let dev = home.path().join("device");
    if let Ok(rd) = std::fs::read_dir(&dev) {
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            files.insert(
                format!("device/{n}"),
                Value::from(std::fs::read_to_string(dev.join(&n)).unwrap()),
            );
        }
    }
    let journal: Vec<Value> = home
        .journal_rows()
        .into_iter()
        .map(|r| json!([r[0], r[1]]))
        .collect();
    let (cache, checks, granted) = auto.state();
    vec![
        format!("out {}", dumps(&Value::Array(outs))),
        format!(
            "log {}",
            dumps(&Value::Array(f.log.lock().unwrap().clone()))
        ),
        format!("journal {}", dumps(&Value::Array(journal))),
        format!("published {}", dumps(&Value::Array(home.events()))),
        format!("files {}", dumps(&Value::Object(files))),
        format!(
            "after {}",
            dumps(
                &json!({"automation_cache": cache, "automation_checks": checks, "tab_granted": granted})
            )
        ),
    ]
}

fn expected(c: &Value) -> Vec<String> {
    ["out", "log", "journal", "published", "files", "after"]
        .iter()
        .map(|k| format!("{k} {}", dumps(&c[*k])))
        .collect()
}

#[test]
fn desktop_matches_python() {
    let fx = fixture("daemon-b/desktop.json");
    let cases = fx["cases"].as_array().unwrap();
    let (mut same, mut diffs) = (0usize, Vec::new());
    for c in cases {
        let (want, got) = (expected(c), run_case(c));
        for (w, g) in want.iter().zip(got.iter()) {
            if w == g {
                same += 1;
            } else {
                diffs.push(format!("{}:\n  python: {w}\n  rust:   {g}", c["name"]));
            }
        }
    }
    eprintln!("desktop: {} cases, {same} comparisons equal", cases.len());
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
    assert!(cases.len() >= 70);
}

/// The window server as the front-window tests see it: TextEdit in front, behind FERB's own app.
struct Native {
    kept: Mutex<Vec<Value>>,
}

impl NativeFront for Native {
    fn windows(&self) -> Vec<Window> {
        vec![
            window(
                &json!({"id": 1, "pid": 900, "bounds": [0.0, 0.0, 300.0, 40.0], "title": "FERB"}),
            ),
            window(
                &json!({"id": 7, "pid": 500, "bounds": [10.0, 20.0, 800.0, 600.0], "title": "Invented notes"}),
            ),
        ]
    }
    fn app_of(&self, pid: i64) -> Option<(String, String)> {
        match pid {
            900 => Some(("FERB".into(), "com.ferb.app".into())),
            500 => Some(("TextEdit".into(), "com.apple.TextEdit".into())),
            _ => None,
        }
    }
    fn parent(&self, _: i64) -> Option<i64> {
        None
    }
    fn main_frame(&self, _: i64) -> Option<Rect> {
        None
    }
    fn is_browser(&self, _: &str) -> bool {
        false
    }
    fn read_sources(&self, bundle: &str) -> Vec<(String, String)> {
        vec![("app".into(), bundle.into())]
    }
    fn redact_image(&self, raw: &[u8]) -> Result<Vec<u8>, String> {
        Ok(raw.to_vec())
    }
    fn excluded(&self, _: &str, _: &str, _: Option<&str>) -> bool {
        false
    }
    fn keep<'a>(
        &'a self,
        request_id: &'a str,
        image: &'a [u8],
        facts: &'a Dict,
        origin: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.kept.lock().unwrap().push(json!([
                request_id,
                String::from_utf8_lossy(image),
                facts,
                origin
            ]));
            Ok("ev-1".to_owned())
        })
    }
}

fn front_fake(mac: Value) -> Fake {
    Fake {
        st: json!({"mac": mac}),
        log: Arc::new(Mutex::new(vec![])),
        probes: Mutex::new(HashMap::new()),
        held: Mutex::new(HashMap::new()),
    }
}

/// desktop.py `_front_window` (L12) wiring: FERB's own window is skipped, the owner's window is shot through the
/// Mac driver and kept as evidence under the request; the screenshot switch withholds the picture; no sources or a
/// paused sense gives None; a capture the caller stops waiting for journals the step it was on (DMN-520).
#[test]
fn front_window_wiring() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let home = Home::new("");
    let native = Native {
        kept: Mutex::new(vec![]),
    };
    let auto = Automation::default();

    let f = front_fake(json!({"jpeg": "shot", "masking": ""}));
    let src = DaemonSources {
        host: &home.host,
        seams: &f,
        native: &native,
        mac: f.mac().unwrap(),
        automation: &auto,
        request_id: "req-1",
        client: None,
        daemon_pid: 4242,
    };
    let got = rt
        .block_on(dk::front_window(&home.host, Some(&src), false))
        .unwrap();
    assert_eq!(
        (got.app.as_str(), got.title.as_str()),
        ("TextEdit", "Invented notes")
    );
    assert_eq!(got.image_ref.as_deref(), Some("ev-1"));
    assert_eq!(got.origin, "private");
    assert_eq!(
        f.log.lock().unwrap().clone(),
        vec![json!(["mac.window_image", "pid:500", "7"])]
    );
    assert_eq!(native.kept.lock().unwrap()[0][0], json!("req-1"));
    assert!(rt
        .block_on(dk::front_window(&home.host, Some(&src), true))
        .is_none());
    assert!(rt
        .block_on(dk::front_window(&home.host, None, false))
        .is_none());

    let off = front_fake(json!({"snapshot_images": false}));
    let src = DaemonSources {
        seams: &off,
        mac: off.mac().unwrap(),
        ..src
    };
    let got = rt
        .block_on(dk::front_window(&home.host, Some(&src), false))
        .unwrap();
    assert_eq!(
        got.withheld.as_deref(),
        Some("screenshots are switched off ([mac] snapshot_image)")
    );
    assert!(got.image_ref.is_none());

    let stuck = front_fake(json!({"image_pending": true}));
    let src = DaemonSources {
        seams: &stuck,
        mac: stuck.mac().unwrap(),
        ..src
    };
    let waited = rt.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            dk::front_window(&home.host, Some(&src), false),
        )
        .await
    });
    assert!(waited.is_err());
    assert_eq!(
        home.journal_rows()
            .last()
            .cloned()
            .map(|r| json!([r[0], r[1]])),
        Some(
            json!(["screen.capture_stalled", {"request_id": "req-1", "stage": "screenshot", "seconds": 0.0}])
        )
    );
}
