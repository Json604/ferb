//! The Mac hands' safety floor on fakes: FERB never operates its own apps, and foreground input goes through right
//! after the owner's Yes in the notch (HAND-238, tests/test_own_apps.py); a file that can run when opened asks with
//! its exact contents unless its editor gets it (HAND-241, tests/test_mac_focus.py); the paths a script names are
//! reported to the protection lists as changes (GATE-223, tests/test_protection.py). Nothing here reads or changes
//! the real Mac: the running apps, Accessibility, the window list and the posted input are stand-ins.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::types::{
    ComputerAction, ComputerActionKind as K, Identity, Mechanism, TerminalGrant, ToolContext,
};
use ferb_mac_hands::apps::RunningApp;
use ferb_mac_hands::ax::{AxTree, El, Rect};
use ferb_mac_hands::driver::{
    Apps, Ax, Facts, MacDriver, Parts, Policy, Shots, Time, Window, OWN_APP, RUNS_OPENED,
};
use ferb_mac_hands::error::{HandError, Result};
use ferb_mac_hands::input::{Cleanup, Gestures, RealInput, Step};
use ferb_mac_hands::locks::GuiLock;
use ferb_mac_hands::tools::MacTools;
use serde_json::json;

const NOTCH: i32 = 10;
const FERB_APP: i32 = 11;
const TERMINAL: i32 = 21;
const FINDER: i32 = 22;
const CODE: i32 = 23;
const TEXTEDIT: i32 = 30;
const OWN_PID: i32 = 999_999;

// ---------- fakes ----------

/// The running apps (FERB's notch and app among them), which one is in front, and every call that would reach macOS.
struct FakeApps {
    running: Vec<RunningApp>,
    front: Mutex<i32>,
    comes_forward: bool,
    calls: Mutex<Vec<String>>,
    /// The process whose element has the keyboard focus (a key non-activating panel's), None: can't tell.
    key_focus: Mutex<Option<i32>>,
}

impl FakeApps {
    fn new(front: i32, comes_forward: bool) -> Self {
        let mut running = vec![
            RunningApp::new("dev.ferb.notch", "FERB Notch", NOTCH, false),
            RunningApp::new("com.ferb.app", "FERB", FERB_APP, false),
            RunningApp::new("com.apple.Terminal", "Terminal", TERMINAL, false),
            RunningApp::new("com.apple.finder", "Finder", FINDER, false),
            RunningApp::new("com.microsoft.VSCode", "Code", CODE, false),
            RunningApp::new("com.apple.TextEdit", "TextEdit", TEXTEDIT, false),
            RunningApp::new("org.python.python", "Python", OWN_PID, false),
        ];
        for (i, b) in [
            "com.apple.JavaLauncher",
            "org.python.PythonLauncher",
            "com.apple.installer",
            "com.apple.shortcuts",
        ]
        .into_iter()
        .enumerate()
        {
            running.push(RunningApp::new(b, b, 40 + i as i32, false));
        }
        FakeApps {
            running,
            front: Mutex::new(front),
            comes_forward,
            calls: Mutex::new(vec![]),
            key_focus: Mutex::new(None),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn ext(path: &str) -> &str {
    path.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

impl Apps for FakeApps {
    fn find(&self, q: &str) -> Option<RunningApp> {
        let m: Vec<&RunningApp> = self
            .running
            .iter()
            .filter(|a| {
                a.bundle_id.to_lowercase() == q.to_lowercase()
                    || a.name == q
                    || format!("pid:{}", a.pid) == q
            })
            .collect();
        (m.len() == 1).then(|| m[0].clone())
    }
    fn pids_of(&self, bundle_ids: &[&str]) -> Vec<i32> {
        self.running
            .iter()
            .filter(|a| {
                bundle_ids
                    .iter()
                    .any(|b| b.to_lowercase() == a.bundle_id.to_lowercase())
            })
            .map(|a| a.pid)
            .collect()
    }
    fn frontmost(&self) -> Option<RunningApp> {
        let front = *self.front.lock().unwrap();
        self.running.iter().find(|a| a.pid == front).cloned()
    }
    fn keyboard_focus_pid(&self) -> Option<i32> {
        *self.key_focus.lock().unwrap()
    }
    fn activate(&self, bundle_id: &str, pid: Option<i32>) -> bool {
        self.calls
            .lock()
            .unwrap()
            .push(format!("activate {bundle_id}"));
        if self.comes_forward {
            *self.front.lock().unwrap() = pid.unwrap_or(0);
        }
        true
    }
    fn open_in(&self, bundle_id: &str, target: &str) -> Option<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("open_in {bundle_id} {target}"));
        None
    }
    /// Launch Services' type facts by extension: these conform to executable, script, installer package,
    /// configuration profile or shortcut.
    fn runs_when_opened(&self, path: &str) -> bool {
        ["jar", "py", "pkg", "mobileconfig", "shortcut", "command"].contains(&ext(path))
    }
    /// VS Code and TextEdit declare themselves the Editor of a .py file; nothing lifts a configuration profile.
    fn edits(&self, bundle_id: &str, path: &str) -> bool {
        ext(path) == "py" && ["com.microsoft.VSCode", "com.apple.TextEdit"].contains(&bundle_id)
    }
    /// Finder's default app: TextEdit for a .py file, Terminal for a .command.
    fn default_app_for(&self, path: &str) -> Option<String> {
        match ext(path) {
            "py" => Some("com.apple.TextEdit".into()),
            "command" => Some("com.apple.Terminal".into()),
            _ => None,
        }
    }
}

/// One app window through Accessibility; the system's hit test answers from `hits`.
struct FakeAx {
    hits: Vec<((f64, f64), i32)>,
}

impl Ax for FakeAx {
    fn read_tree(
        &self,
        _pid: i32,
        _b: bool,
        _w: Option<&El>,
        _o: usize,
        _bs: Option<f64>,
        _m: bool,
    ) -> AxTree {
        AxTree {
            secure: Some(false),
            ..Default::default()
        }
    }
    fn expose_tree(&self, _pid: i32) -> bool {
        true
    }
    fn app_window(&self, _pid: i32) -> Option<El> {
        Some(El::new("win".to_string()))
    }
    fn ax_windows(&self, _pid: i32) -> Option<Vec<El>> {
        Some(vec![El::new("win".to_string())])
    }
    fn frame(&self, _el: &El) -> Option<Rect> {
        Some((0.0, 0.0, 400.0, 300.0))
    }
    fn press(&self, _el: &El) -> Result<bool> {
        panic!("nothing is pressed here")
    }
    fn focus(&self, _el: &El) -> bool {
        panic!("nothing is focused here")
    }
    fn has_focus(&self, _pid: i32, _el: &El) -> Option<bool> {
        None
    }
    fn set_value(&self, _el: &El, _t: &str) -> Result<bool> {
        panic!("nothing is set here")
    }
    fn value(&self, _el: &El) -> Option<String> {
        None
    }
    fn menu_item_for_shortcut(
        &self,
        _pid: i32,
        _key: &str,
        _mods: &[String],
    ) -> Option<(El, String, bool)> {
        None
    }
    fn hit(&self, x: f64, y: f64) -> Option<(i32, Option<Rect>)> {
        self.hits
            .iter()
            .find(|(p, _)| *p == (x, y))
            .map(|(_, pid)| (*pid, None))
    }
}

/// No window list: a capture or a list read is never expected here.
struct NoShots;

impl Shots for NoShots {
    fn window(&self, _id: i64) -> Option<Window> {
        None
    }
    fn front_window(&self, _pid: i32, _like: Option<Rect>, _title: Option<&str>) -> Option<Window> {
        None
    }
    fn on_screen(&self, _id: i64) -> bool {
        true
    }
    fn window_at(&self, _x: f64, _y: f64) -> Option<Window> {
        None
    }
    fn capture(&self, _w: &Window, _m: Option<u32>, _p: bool) -> Result<(Vec<u8>, i64, i64)> {
        panic!("nothing is captured here")
    }
}

/// Keys and typing recorded as they would be posted; nothing else is expected.
struct Posted(Arc<Mutex<Vec<String>>>);

impl Posted {
    fn step(&self, what: String) -> Vec<Step> {
        let posted = self.0.clone();
        vec![Step::new(what.clone(), move || {
            posted.lock().unwrap().push(what.clone());
            Ok(())
        })]
    }
}

impl Gestures for Posted {
    fn click_steps(&self, _x: f64, _y: f64, _pid: Option<i32>) -> Vec<Step> {
        self.step("click".into())
    }
    fn type_steps(&self, t: &str, _pid: Option<i32>) -> Vec<Step> {
        self.step(format!("type {t}"))
    }
    fn key_steps(&self, c: &str, _pid: Option<i32>) -> Vec<Step> {
        self.step(format!("key {c}"))
    }
    fn wheel_steps(
        &self,
        _x: f64,
        _y: f64,
        _l: i32,
        _p: Option<i32>,
        _h: bool,
        _w: Option<u32>,
        _r: bool,
        _px: bool,
        _o: (f64, f64),
    ) -> Vec<Step> {
        panic!("no wheel")
    }
    fn pointer_steps(
        &self,
        _s: (f64, f64),
        _e: (f64, f64),
        _k: &str,
        _p: i32,
        _w: Option<u32>,
        _r: bool,
        _o: (f64, f64),
    ) -> Result<(Vec<Step>, Cleanup)> {
        panic!("no pointer")
    }
}

/// The policy module's answers for apps that are no terminals; Finder hands a file to its default app.
struct Plain;

impl Policy for Plain {
    fn surface(&self, _b: &str, _c: &[Facts], _o: &[String], _s: bool, _u: bool) -> Option<String> {
        None
    }
    fn can_host(&self, _b: &str, _o: &[String], _s: bool) -> bool {
        false
    }
    fn run_menu(&self, _p: &[String]) -> bool {
        false
    }
    fn terminal_menu(&self, _t: &str) -> bool {
        false
    }
    fn run_control(&self, _c: &[Facts], _e: bool) -> Option<String> {
        None
    }
    fn editor_app(&self, _b: &str) -> bool {
        false
    }
    fn structural(&self, _f: &Facts) -> bool {
        false
    }
    fn newline(&self, _t: &str) -> bool {
        false
    }
    fn code_runner(&self, _b: &str) -> Option<String> {
        None
    }
    fn opens_files(&self, b: &str) -> bool {
        b == "com.apple.finder"
    }
    fn unasked(&self, why: &str, _a: &str) -> String {
        why.to_string()
    }
    fn browser_page(&self, _a: Option<&str>) -> bool {
        false
    }
    fn typed_destination(&self, _t: &str) -> Option<String> {
        None
    }
}

/// A paused clock: a sleep moves it on at once.
#[derive(Default)]
struct PausedTime(Mutex<f64>);

impl Time for PausedTime {
    fn monotonic(&self) -> f64 {
        *self.0.lock().unwrap()
    }
    fn sleep(&self, s: f64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        *self.0.lock().unwrap() += s;
        Box::pin(async {})
    }
}

struct Mac {
    driver: Arc<MacDriver>,
    apps: Arc<FakeApps>,
    posted: Arc<Mutex<Vec<String>>>,
}

fn mac(front: i32, comes_forward: bool, hits: Vec<((f64, f64), i32)>) -> Mac {
    let apps = Arc::new(FakeApps::new(front, comes_forward));
    let posted = Arc::new(Mutex::new(vec![]));
    let mut input = RealInput::new(Arc::new(|| false), GuiLock::new());
    input.sleep = Arc::new(|_| Box::pin(async {}));
    let mut d = MacDriver::new(Parts {
        device_id: "dev".into(),
        input,
        gestures: Arc::new(Posted(posted.clone())),
        ax: Arc::new(FakeAx { hits }),
        apps: apps.clone(),
        shots: Arc::new(NoShots),
        policy: Arc::new(Plain),
        time: Arc::new(PausedTime::default()),
    });
    d.own_pid = OWN_PID;
    Mac {
        driver: Arc::new(d),
        apps,
        posted,
    }
}

fn tools(d: &Arc<MacDriver>) -> MacTools {
    MacTools::new(d.clone(), Arc::new(|j, _m, _b, _s| Ok(j)))
}

fn ctx(terminal: Option<TerminalGrant>) -> ToolContext {
    ToolContext {
        task_id: "t1".into(),
        worker_id: "w1".into(),
        request_id: "r1".into(),
        identity: Identity::Owner,
        scopes: ["mac:read", "mac:act"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        device_id: "dev".into(),
        span_id: None,
        workspace: String::new(),
        owner_roots: vec![],
        untrusted_roots: vec![],
        intent_id: None,
        observe_only: false,
        probe: false,
        path_closed: None,
        escape: None,
        terminal,
        folders: vec![],
    }
}

fn foreground(kind: K, text: &str) -> ComputerAction {
    ComputerAction {
        kind,
        element: None,
        text: Some(text.into()),
        select: serde_json::Value::Null,
        direction: None,
        amount: None,
        window: None,
        allow_foreground: true,
        mechanism: Some(Mechanism::Foreground),
        name: None,
    }
}

fn refusal(r: Result<impl std::fmt::Debug>) -> String {
    match r {
        Err(HandError::Refused(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ---------- HAND-238: FERB's own apps ----------

// decision: HAND-238
#[tokio::test]
async fn every_mac_tool_refuses_ferbs_own_apps_and_its_own_process() {
    let m = mac(CODE, true, vec![]);
    let d = &m.driver;
    for name in [
        "dev.ferb.notch",
        "FERB",
        "pid:11",
        "DEV.FERB.NOTCH",
        &format!("pid:{OWN_PID}"),
    ] {
        assert_eq!(refusal(d.app(name)), OWN_APP, "{name}");
    }
    let press = ComputerAction {
        mechanism: Some(Mechanism::Ax),
        allow_foreground: false,
        element: Some("s1e1".into()),
        text: None,
        ..foreground(K::Press, "")
    };
    assert_eq!(
        refusal(d.act("FERB", &press, "w1", Some("t1"), None).await),
        OWN_APP
    );
    assert_eq!(
        refusal(
            d.act(
                "dev.ferb.notch",
                &foreground(K::Key, "return"),
                "w1",
                Some("t1"),
                None
            )
            .await
        ),
        OWN_APP
    );
    let open = tools(d)
        .run(
            "mac.open",
            &json!({"bundle_id": "dev.ferb.notch"}),
            &ctx(None),
        )
        .await
        .unwrap();
    assert!(open.is_error, "{:?}", open.content);
    assert!(m.posted.lock().unwrap().is_empty() && m.apps.calls().is_empty());
    assert_eq!(d.app("Code").unwrap().bundle_id, "com.microsoft.VSCode"); // any other app is unaffected
}

// decision: HAND-238
#[test]
fn a_real_click_a_ferb_window_would_take_is_refused() {
    // the system's hit test: the notch covers (200, 10); VS Code is under (50, 200)
    let m = mac(
        CODE,
        true,
        vec![((200.0, 10.0), NOTCH), ((50.0, 200.0), CODE)],
    );
    assert_eq!(
        m.driver
            .own_point(200.0, 10.0, Some(CODE), Some(4115))
            .as_deref(),
        Some(OWN_APP)
    );
    assert_eq!(
        m.driver.own_point(50.0, 200.0, Some(CODE), Some(4115)),
        None
    );
}

// decision: HAND-238
#[tokio::test]
async fn foreground_input_right_after_a_yes_in_the_notch_goes_through() {
    // the owner just answered in the notch: FERB is in front; the target is brought forward and the input lands there
    let m = mac(NOTCH, true, vec![]);
    for (kind, text) in [(K::Key, "return"), (K::Key, "cmd+s")] {
        *m.apps.front.lock().unwrap() = NOTCH;
        let r = m
            .driver
            .act("TextEdit", &foreground(kind, text), "w1", Some("t1"), None)
            .await
            .unwrap();
        assert!(r.ok, "{}", r.detail);
    }
    assert_eq!(*m.posted.lock().unwrap(), ["key return", "key cmd+s"]);
    assert_eq!(
        m.apps.calls(),
        ["activate com.apple.TextEdit", "activate com.apple.TextEdit"]
    );
}

// decision: HAND-238
#[tokio::test]
async fn foreground_input_never_lands_while_the_notch_stays_in_front() {
    let m = mac(NOTCH, false, vec![]);
    let r = m
        .driver
        .act(
            "TextEdit",
            &foreground(K::Key, "return"),
            "w1",
            Some("t1"),
            None,
        )
        .await;
    assert!(matches!(r, Err(HandError::PointerInterrupted(_))), "{r:?}");
    assert!(m.posted.lock().unwrap().is_empty());
}

// HAND-571: the target is frontmost, but FERB's non-activating panel (a question's box) holds the keyboard focus:
// nothing is posted, so no key or typed text can land in it; with the focus in the target the same input goes through.
// decision: HAND-571
#[tokio::test]
async fn foreground_input_never_types_while_a_ferb_panel_holds_the_keyboard_focus() {
    let m = mac(TEXTEDIT, true, vec![]);
    *m.apps.key_focus.lock().unwrap() = Some(NOTCH);
    // a Return that would send the box, a paste into it
    for (kind, text) in [(K::Key, "return"), (K::Key, "cmd+v")] {
        let r = m
            .driver
            .act("TextEdit", &foreground(kind, text), "w1", Some("t1"), None)
            .await;
        match r {
            Err(HandError::PointerInterrupted(why)) => {
                assert_eq!(why, ferb_mac_hands::driver::OWN_FOCUS)
            }
            other => panic!("{text}: {other:?}"),
        }
    }
    assert!(m.posted.lock().unwrap().is_empty());
    *m.apps.key_focus.lock().unwrap() = Some(TEXTEDIT);
    let r = m
        .driver
        .act(
            "TextEdit",
            &foreground(K::Key, "return"),
            "w1",
            Some("t1"),
            None,
        )
        .await
        .unwrap();
    assert!(r.ok, "{}", r.detail);
    assert_eq!(*m.posted.lock().unwrap(), ["key return"]);
}

// ---------- HAND-241: a file that can run when opened ----------

// decision: HAND-241
#[tokio::test]
async fn a_file_whose_type_runs_when_opened_asks_with_its_bytes_in_any_app() {
    let dir = tempfile::tempdir().unwrap();
    for (name, app) in [
        ("tool.jar", "com.apple.JavaLauncher"),
        ("run.py", "org.python.PythonLauncher"),
        ("setup.pkg", "com.apple.installer"),
        ("wifi.mobileconfig", "com.apple.TextEdit"),
        ("go.shortcut", "com.apple.shortcuts"),
    ] {
        let m = mac(CODE, true, vec![]);
        let t = tools(&m.driver);
        let f = dir.path().join(name);
        let path = f.to_str().unwrap().to_string();
        std::fs::write(&f, b"PK\x03\x04\xff payload").unwrap();
        let args = json!({"bundle_id": app, "path": path});
        let inp = t
            .terminal_input("mac.open", &args, "t1")
            .await
            .unwrap_or_else(|| panic!("{name} in {app} asks"));
        assert_eq!(inp.surface, RUNS_OPENED);
        assert_eq!(inp.files.len(), 1);
        assert_eq!(inp.files[0].0, path);
        assert_eq!(inp.files[0].2, "(a binary file of 13 bytes)");
        // without the owner's Yes nothing opens
        let out = t.run("mac.open", &args, &ctx(None)).await.unwrap();
        assert!(out.is_error, "{name}: {:?}", out.content);
        let yes = TerminalGrant {
            input: inp,
            confirmation_id: "c1".into(),
        };
        // the Yes covers only the bytes the owner saw
        std::fs::write(&f, b"changed").unwrap();
        let out = t
            .run("mac.open", &args, &ctx(Some(yes.clone())))
            .await
            .unwrap();
        assert!(out.is_error, "{name}: {:?}", out.content);
        assert!(m.apps.calls().is_empty(), "{name}: {:?}", m.apps.calls());
        std::fs::write(&f, b"PK\x03\x04\xff payload").unwrap();
        let out = t.run("mac.open", &args, &ctx(Some(yes))).await.unwrap();
        assert!(!out.is_error, "{name}: {:?}", out.content);
        assert_eq!(m.apps.calls(), [format!("open_in {app} {path}")]);
    }
}

// decision: HAND-241
#[tokio::test]
async fn a_runnable_file_its_declared_editor_gets_does_not_ask() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("run.py");
    std::fs::write(&script, "import os\n").unwrap();
    let script = script.to_str().unwrap().to_string();
    let command = dir.path().join("clean.command");
    std::fs::write(&command, "rm -rf ~/Downloads/x\n").unwrap();
    let command = command.to_str().unwrap().to_string();
    let m = mac(CODE, true, vec![]);
    let t = tools(&m.driver);
    let asks = |app: &str, path: &str| {
        let t = &t;
        let args = json!({"bundle_id": app, "path": path});
        async move { t.terminal_input("mac.open", &args, "t1").await }
    };
    // VS Code edits it; Finder hands it to its default app, TextEdit, which edits it too
    assert!(asks("com.microsoft.VSCode", &script).await.is_none());
    assert!(asks("com.apple.finder", &script).await.is_none());
    // a launcher still asks; Finder's default for a .command runs it, and the question shows its text
    assert!(asks("org.python.PythonLauncher", &script).await.is_some());
    let inp = asks("com.apple.finder", &command).await.unwrap();
    assert!(inp.files[0].2.contains("rm -rf"), "{:?}", inp.files);
    let out = t
        .run(
            "mac.open",
            &json!({"bundle_id": "com.microsoft.VSCode", "path": script}),
            &ctx(None),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{:?}", out.content);
    assert_eq!(
        m.apps.calls(),
        [format!("open_in com.microsoft.VSCode {script}")]
    );
}

// ---------- GATE-223: the paths a script names ----------

// decision: GATE-223
#[test]
fn every_path_a_script_names_is_reported_as_a_change() {
    let m = mac(CODE, true, vec![]);
    let t = tools(&m.driver);
    let script = "duplicate file (POSIX file \"/tmp/x\") to folder (POSIX file \"/Users/invented/.ferb\")\n\
                  make new file at folder \"/Users/invented/Library/LaunchAgents\" with properties {name:\"x\"}\n\
                  move it to \"/Users/invented/Library/LaunchAgents/com.ferb.daemon.plist\"\n\
                  set f to \"nohd:not a volume\"";
    let paths = t.file_paths(
        "mac.applescript",
        &json!({"app": "com.apple.finder", "script": script}),
    );
    assert_eq!(
        paths,
        [
            ("/tmp/x".to_string(), "write"),
            ("/Users/invented/.ferb".to_string(), "write"),
            ("/Users/invented/Library/LaunchAgents".to_string(), "write"),
            (
                "/Users/invented/Library/LaunchAgents/com.ferb.daemon.plist".to_string(),
                "write"
            ),
        ]
    );
}
