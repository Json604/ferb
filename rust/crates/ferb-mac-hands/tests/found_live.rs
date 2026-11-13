//! Guards for what the first runs on a real Mac found (rust/JOURNAL.md, entries 45 and 46), on fakes: how an app is
//! opened (in the background: `open -g -b`, NSWorkspace with `activates` off, never an activation), an app the
//! running-app list shows only a moment after its launch (macOS updates NSWorkspace's list on a later turn of the
//! main run loop), and which window a capture with no window id takes (driver.py:913-923 and screenshot.py:88-99:
//! the sole reachable window, else none). Nothing here reads or changes the real Mac.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::types::{Identity, ToolContext};
use ferb_mac_hands::apps::RunningApp;
use ferb_mac_hands::ax::{AxTree, El, Rect};
use ferb_mac_hands::driver::{Apps, Ax, Facts, MacDriver, Parts, Policy, Shots, Time, Window};
use ferb_mac_hands::error::{HandError, Result};
use ferb_mac_hands::input::{Cleanup, Gestures, RealInput, Step};
use ferb_mac_hands::launched::{AppControl, LaunchedApps};
use ferb_mac_hands::locks::GuiLock;
use ferb_mac_hands::shots::WindowList;
use ferb_mac_hands::tools::MacTools;
use serde_json::{json, Value};

const TE: &str = "com.apple.TextEdit";

// ---------- fakes ----------

/// An app that isn't running until a launch or an open starts it, and that the list shows only after `late` more
/// looks (the main run loop's turn that macOS needs before NSWorkspace lists it). Every call is recorded.
struct LateApps {
    calls: Mutex<Vec<String>>,
    running: Mutex<Vec<RunningApp>>,
    late: Mutex<Option<usize>>,
    after: usize,
}

impl LateApps {
    fn new(after: usize) -> Self {
        LateApps {
            calls: Mutex::new(vec![]),
            running: Mutex::new(vec![RunningApp::new(
                "com.microsoft.VSCode",
                "Code",
                23,
                true,
            )]),
            late: Mutex::new(None),
            after,
        }
    }
    fn started(&self) {
        *self.late.lock().unwrap() = Some(self.after);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Apps for LateApps {
    fn find(&self, q: &str) -> Option<RunningApp> {
        let mut late = self.late.lock().unwrap();
        if let Some(n) = late.as_mut() {
            if *n == 0 {
                let mut r = self.running.lock().unwrap();
                if !r.iter().any(|a| a.bundle_id == TE) {
                    r.push(RunningApp::new(TE, "TextEdit", 77, false));
                }
            } else {
                *n -= 1;
            }
        }
        let r = self.running.lock().unwrap();
        let m: Vec<RunningApp> = r
            .iter()
            .filter(|a| a.bundle_id == q || a.name == q || format!("pid:{}", a.pid) == q)
            .cloned()
            .collect();
        (m.len() == 1).then(|| m[0].clone())
    }
    fn frontmost(&self) -> Option<RunningApp> {
        self.running
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.active)
            .cloned()
    }
    fn activate(&self, bundle_id: &str, _pid: Option<i32>) -> bool {
        self.calls
            .lock()
            .unwrap()
            .push(format!("activate {bundle_id}"));
        true
    }
    fn launch(&self, bundle_id: &str) -> bool {
        self.calls
            .lock()
            .unwrap()
            .push(format!("launch {bundle_id}"));
        self.started();
        true
    }
    fn open_in(&self, bundle_id: &str, target: &str) -> Option<String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("open_in {bundle_id} {target}"));
        self.started();
        None
    }
}

/// One app window through Accessibility at `frame`, or (`None`) more than one, so no sole reachable window.
struct WindowsAx {
    frame: Option<Rect>,
}

impl Ax for WindowsAx {
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
        self.frame.map(|_| El::new("win".to_string()))
    }
    fn ax_windows(&self, _pid: i32) -> Option<Vec<El>> {
        Some(vec![El::new("win".to_string())])
    }
    fn frame(&self, _el: &El) -> Option<Rect> {
        self.frame
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
}

/// The window server's list, invented.
struct Listed(Vec<Window>);

impl WindowList for Listed {
    fn list(&self, _everywhere: bool) -> Option<Vec<Window>> {
        Some(self.0.clone())
    }
}

/// Shots over an invented window list (screenshot.py's rules, `shots::*`), recording what is captured.
struct ListShots {
    list: Listed,
    captured: Mutex<Vec<i64>>,
}

impl Shots for ListShots {
    fn window(&self, id: i64) -> Option<Window> {
        self.list.0.iter().find(|w| w.id == id).cloned()
    }
    fn front_window(&self, pid: i32, like: Option<Rect>, title: Option<&str>) -> Option<Window> {
        ferb_mac_hands::shots::front_window(&self.list, pid, like, title)
    }
    fn on_screen(&self, id: i64) -> bool {
        self.list.0.iter().any(|w| w.id == id)
    }
    fn window_at(&self, _x: f64, _y: f64) -> Option<Window> {
        None
    }
    fn capture(&self, w: &Window, _m: Option<u32>, _p: bool) -> Result<(Vec<u8>, i64, i64)> {
        self.captured.lock().unwrap().push(w.id);
        Ok((b"\xff\xd8\xffJPEG".to_vec(), 100, 100))
    }
}

/// No input and no policy question is expected in these tests: any would fail them.
struct NoInput;

impl Gestures for NoInput {
    fn click_steps(&self, _x: f64, _y: f64, _pid: Option<i32>) -> Vec<Step> {
        panic!("no click")
    }
    fn type_steps(&self, _t: &str, _pid: Option<i32>) -> Vec<Step> {
        panic!("no typing")
    }
    fn key_steps(&self, _c: &str, _pid: Option<i32>) -> Vec<Step> {
        panic!("no key")
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

struct NoPolicy;

impl Policy for NoPolicy {
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
    fn opens_files(&self, _b: &str) -> bool {
        false
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

struct Control;

impl AppControl for Control {
    fn running(&self) -> Vec<RunningApp> {
        vec![]
    }
    fn unsaved(&self, _pid: i32) -> Option<bool> {
        Some(false)
    }
    fn quit(&self, _pid: i32, _b: &str) -> bool {
        true
    }
    fn hide(&self, _pid: i32, _b: &str) -> bool {
        true
    }
}

fn driver(
    apps: Arc<dyn Apps>,
    ax: WindowsAx,
    windows: Vec<Window>,
) -> (Arc<MacDriver>, Arc<ListShots>) {
    let shots = Arc::new(ListShots {
        list: Listed(windows),
        captured: Mutex::new(vec![]),
    });
    let mut input = RealInput::new(Arc::new(|| false), GuiLock::new());
    input.sleep = Arc::new(|_| Box::pin(async {}));
    let mut d = MacDriver::new(Parts {
        device_id: "dev".into(),
        input,
        gestures: Arc::new(NoInput),
        ax: Arc::new(ax),
        apps,
        shots: shots.clone(),
        policy: Arc::new(NoPolicy),
        time: Arc::new(PausedTime::default()),
    });
    d.own_pid = 999_999;
    d.launched = Some(Arc::new(LaunchedApps::new(
        Arc::new(Control),
        Arc::new(|_, _| false),
        Arc::new(|| "2026-11-13T00:00:00+00:00".to_string()),
    )));
    (Arc::new(d), shots)
}

fn ctx() -> ToolContext {
    ToolContext {
        task_id: "t1.worker".into(),
        worker_id: "w1".into(),
        request_id: "r1".into(),
        identity: Identity::Owner,
        scopes: ["mac:read", "mac:act", "screen"]
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
        terminal: None,
        folders: vec![],
    }
}

const FRAME: Rect = (100.0, 80.0, 900.0, 600.0);

fn tools(d: &Arc<MacDriver>) -> MacTools {
    MacTools::new(d.clone(), Arc::new(|j, _m, _b, _s| Ok(j)))
}

// ---------- mac.open: in the background, and an app the list shows late is still the task's ----------

#[test]
fn launching_is_open_g_b_and_opening_never_activates() {
    // apps.py:151 (HAND-239): launched in the background, not hidden; apps.py:257 (HAND-240): activates off
    assert_eq!(
        ferb_mac_hands::apps::launch_args(TE),
        ["-g", "-b", TE].map(String::from).to_vec()
    );
    // the constant native::apps hands to NSWorkspaceOpenConfiguration: guarded against an edit that turns it on
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(!ferb_mac_hands::apps::OPEN_ACTIVATES);
    }
}

#[tokio::test]
async fn an_app_listed_only_after_its_launch_is_still_recorded_as_the_tasks() {
    // decision: HAND-239
    for (args, call) in [
        (json!({"bundle_id": TE}), format!("launch {TE}")),
        (
            json!({"bundle_id": TE, "url": "https://example.com/a"}),
            format!("open_in {TE} https://example.com/a"),
        ),
    ] {
        let apps = Arc::new(LateApps::new(3));
        let (d, _) = driver(apps.clone(), WindowsAx { frame: Some(FRAME) }, vec![]);
        let out = tools(&d).run("mac.open", &args, &ctx()).await.unwrap();
        let content = Value::Object(out.content.clone());
        assert_eq!(content["launched"], json!(true), "{content}");
        assert_eq!(content["window"], json!(true), "{content}");
        // the only call that reached macOS opened it; nothing brought it forward
        assert_eq!(apps.calls(), vec![call]);
        let recorded = d.launched.as_ref().unwrap().of("t1.worker");
        assert_eq!(recorded.len(), 1);
        assert_eq!((recorded[0].bundle_id.as_str(), recorded[0].pid), (TE, 77));
    }
}

#[tokio::test]
async fn an_app_already_running_is_not_the_tasks() {
    let apps = Arc::new(LateApps::new(0));
    apps.started();
    let _ = apps.find(TE);
    let (d, _) = driver(apps.clone(), WindowsAx { frame: Some(FRAME) }, vec![]);
    let out = tools(&d)
        .run("mac.open", &json!({"bundle_id": TE}), &ctx())
        .await
        .unwrap();
    assert_eq!(out.content["launched"], json!(false));
    assert!(d.launched.as_ref().unwrap().of("t1.worker").is_empty());
}

// ---------- the window a capture takes with no window id ----------

fn w(id: i64, pid: i32, bounds: Rect) -> Window {
    Window::new(id, pid, bounds, "")
}

#[test]
fn front_window_is_the_one_at_the_ax_frame_or_the_sole_one() {
    let mut menu = w(5, 7, FRAME);
    menu.layer = 25; // a menu or overlay sits above layer 0
    let mut clear = w(6, 7, (0.0, 0.0, 50.0, 50.0));
    clear.alpha = 0.0;
    let list = Listed(vec![
        menu,
        clear,
        w(1, 7, FRAME),
        w(2, 7, (1.0, 0.0, 400.0, 300.0)),
        w(3, 8, FRAME),
    ]);
    let pick =
        |like, title| ferb_mac_hands::shots::front_window(&list, 7, like, title).map(|w| w.id);
    // with no frame: two normal windows of the process, so none (never by order or size)
    assert_eq!(pick(None, None), None);
    // the Accessibility window's frame, within 2 points
    assert_eq!(pick(Some(FRAME), None), Some(1));
    assert_eq!(pick(Some((101.5, 81.0, 899.0, 601.0)), None), Some(1));
    assert_eq!(pick(Some((103.0, 80.0, 900.0, 600.0)), None), None);
    // the sole normal window of a process
    assert_eq!(
        ferb_mac_hands::shots::front_window(&list, 8, None, None).map(|w| w.id),
        Some(3)
    );
    // windows sharing the bounds: the title picks one
    let maxed = Listed(vec![
        Window::new(1, 7, FRAME, "Inbox"),
        Window::new(2, 7, FRAME, "Draft  one"),
    ]);
    let by = |t| ferb_mac_hands::shots::front_window(&maxed, 7, Some(FRAME), t).map(|w| w.id);
    assert_eq!(by(None), None);
    assert_eq!(by(Some("Draft one")), Some(2));
}

#[tokio::test]
async fn a_capture_without_a_window_id_needs_a_sole_reachable_window() {
    // decision: HAND-220
    let windows = vec![w(1, 77, FRAME), w(2, 77, (0.0, 0.0, 300.0, 200.0))];
    let running = || {
        let apps = Arc::new(LateApps::new(0));
        apps.started();
        apps
    };
    let paint = |j: Vec<u8>, _m: &[Rect], _b: Rect, _s: (i64, i64)| Ok(j);
    // several windows and no sole reachable one: NoTarget, nothing captured (driver.py:921-923)
    let (d, shots) = driver(running(), WindowsAx { frame: None }, windows.clone());
    match d.screenshot(TE, None, false, &paint).await {
        Err(HandError::NoTarget(m)) => assert!(m.contains("no unique capture target"), "{m}"),
        other => panic!("{:?}", other.map(|r| r.1.window)),
    }
    assert!(shots.captured.lock().unwrap().is_empty());
    // the sole reachable window: the listed window at its frame
    let (d, shots) = driver(running(), WindowsAx { frame: Some(FRAME) }, windows.clone());
    let (_, shot, _) = d.screenshot(TE, None, false, &paint).await.unwrap();
    assert_eq!(shot.window, 1);
    assert_eq!(*shots.captured.lock().unwrap(), vec![1]);
    // an exact window id from the snapshot's list: that window
    let (d, shots) = driver(running(), WindowsAx { frame: None }, windows);
    let (_, shot, _) = d.screenshot(TE, Some("2"), false, &paint).await.unwrap();
    assert_eq!(shot.window, 2);
    assert_eq!(*shots.captured.lock().unwrap(), vec![2]);
}
