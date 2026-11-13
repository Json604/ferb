//! The driver and the tools against what the Python build did on the same fakes (decision: ENG-3).
//! rust/fixtures/mac-hands/make_driver.py drove the real MacComputerDriver and Mac tools through scripted scenarios
//! on tests/mackit.py's fakes and recorded every step; the fakes here are those fakes in Rust (the same state, the
//! same answers, the same recorded gestures), and the policy module's answers are replayed from what Python's gave.
//! A step where Python's fake lacked a method (AttributeError) is a limit of that fake, not of the driver, and is
//! skipped. Nothing here posts an event, captures a window or reads the real Mac.

mod common;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use ferb_core::types::{
    ComputerAction, ComputerActionKind as K, Identity, Mechanism, TerminalGrant, ToolContext,
};
use ferb_mac_hands::apps::RunningApp;
use ferb_mac_hands::ax::{AxNode, AxTree, El, Rect};
use ferb_mac_hands::driver::{Apps, Ax, Facts, MacDriver, Parts, Policy, Shots, Time, Window};
use ferb_mac_hands::error::{HandError, Result};
use ferb_mac_hands::input::{Cleanup, Gestures, RealInput, Step};
use ferb_mac_hands::locks::GuiLock;
use ferb_mac_hands::profile::Profiles;
use ferb_mac_hands::tools::MacTools;
use serde_json::{json, Map, Value};

fn el(s: &str) -> El {
    El::new(s.to_string())
}
fn id(e: &El) -> String {
    e.get::<String>().cloned().unwrap_or_default()
}
fn pyf(x: f64) -> String {
    ferb_py::json::python_float(x)
}
fn repr(s: &str) -> String {
    ferb_mac_hands::driver::py_repr(s)
}

// ---------- the chat app (mackit.Ax, Apps; test_mac_hands.FactsAx) ----------

#[derive(Default)]
struct ChatState {
    takes_background: bool,
    box_has_focus: Option<bool>,
    press_ok: bool,
    send_button: bool,
    value: String,
    text: String,
    calls: Vec<String>,
    facts: bool,
    focus_el: String,
    chains: HashMap<String, Vec<Facts>>,
}

struct ChatAx(Arc<Mutex<ChatState>>);

impl Ax for ChatAx {
    fn read_tree(
        &self,
        _pid: i32,
        _b: bool,
        _w: Option<&El>,
        _o: usize,
        _bs: Option<f64>,
        _m: bool,
    ) -> AxTree {
        let s = self.0.lock().unwrap();
        let mut nodes = vec![AxNode::new(
            el("box"),
            "textbox",
            "Message",
            Some(&s.value),
            (0.0, 0.0, 10.0, 10.0),
            false,
            true,
        )];
        if s.send_button {
            nodes.push(AxNode::new(
                el("send"),
                "button",
                "Send",
                None,
                (20.0, 0.0, 10.0, 10.0),
                false,
                true,
            ));
        }
        AxTree::of(nodes, &s.text, "Chat")
    }
    fn expose_tree(&self, _pid: i32) -> bool {
        self.0.lock().unwrap().calls.push("expose".into());
        true
    }
    fn app_window(&self, _pid: i32) -> Option<El> {
        Some(el("win"))
    }
    fn window_title(&self, _w: &El) -> String {
        "Chat".into()
    }
    fn value(&self, e: &El) -> Option<String> {
        let s = self.0.lock().unwrap();
        if id(e) == "box" {
            Some(s.value.clone())
        } else {
            None
        }
    }
    fn has_focus(&self, _pid: i32, _e: &El) -> Option<bool> {
        self.0.lock().unwrap().box_has_focus
    }
    fn focus(&self, _e: &El) -> bool {
        self.0.lock().unwrap().calls.push("focus".into());
        true
    }
    fn set_value(&self, _e: &El, _t: &str) -> Result<bool> {
        Ok(false)
    }
    fn press(&self, _e: &El) -> Result<bool> {
        let mut s = self.0.lock().unwrap();
        s.calls.push("press".into());
        Ok(s.press_ok)
    }
    fn cancel(&self, e: &El) -> Result<bool> {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("cancel {}", id(e)));
        Ok(true)
    }
    fn center(&self, _n: &AxNode) -> (f64, f64) {
        (5.0, 5.0)
    }
    fn menu_item_for_shortcut(
        &self,
        _pid: i32,
        _k: &str,
        _m: &[String],
    ) -> Option<(El, String, bool)> {
        None
    }
    // FactsAx
    fn role_of(&self, e: &El) -> String {
        if !self.0.lock().unwrap().facts {
            return String::new();
        }
        match id(e).as_str() {
            "box" => "AXTextField",
            "send" => "AXButton",
            "pw" => "AXSecureTextField",
            _ => "",
        }
        .into()
    }
    fn subrole_of(&self, e: &El) -> String {
        if self.0.lock().unwrap().facts && id(e) == "pw" {
            "AXSecureTextField".into()
        } else {
            String::new()
        }
    }
    fn secure_element(&self, e: &El) -> bool {
        self.0.lock().unwrap().facts && id(e) == "pw"
    }
    fn focused_element(&self, _pid: i32) -> Option<El> {
        let s = self.0.lock().unwrap();
        if s.facts {
            Some(el(&s.focus_el))
        } else {
            None
        }
    }
    fn surface_facts(&self, e: &El) -> Option<Vec<Facts>> {
        let s = self.0.lock().unwrap();
        if s.chains.is_empty() {
            None
        } else {
            Some(s.chains.get(&id(e)).cloned().unwrap_or_default())
        }
    }
    fn menu_item_shortcut(&self, _pid: i32, path: &[String]) -> Option<(String, i64)> {
        if !self.0.lock().unwrap().chains.is_empty() && path == ["Edit", "Paste"] {
            Some(("V".into(), 0))
        } else {
            None
        }
    }
    fn place_of(&self, e: &El) -> (String, String) {
        if self.0.lock().unwrap().facts {
            ("Chat with Asha".into(), id(e))
        } else {
            (String::new(), String::new())
        }
    }
}

struct ChatApps(Mutex<Vec<RunningApp>>);

fn find_in(apps: &[RunningApp], q: &str) -> Option<RunningApp> {
    let q = q.to_lowercase();
    apps.iter()
        .find(|a| {
            q == a.bundle_id.to_lowercase()
                || q == a.name.to_lowercase()
                || q == format!("pid:{}", a.pid)
        })
        .cloned()
}

impl Apps for ChatApps {
    fn find(&self, q: &str) -> Option<RunningApp> {
        find_in(&self.0.lock().unwrap(), q)
    }
    fn frontmost(&self) -> Option<RunningApp> {
        self.0.lock().unwrap().iter().find(|a| a.active).cloned()
    }
    fn activate(&self, bid: &str, _pid: Option<i32>) -> bool {
        let mut apps = self.0.lock().unwrap();
        for a in apps.iter_mut() {
            a.active = a.bundle_id == bid;
        }
        true
    }
}

struct ChatGestures(Arc<Mutex<ChatState>>, Arc<Mutex<Vec<String>>>);

impl Gestures for ChatGestures {
    fn click_steps(&self, _x: f64, _y: f64, _pid: Option<i32>) -> Vec<Step> {
        let posted = self.1.clone();
        vec![Step::new("click", move || {
            posted.lock().unwrap().push("click foreground".into());
            Ok(())
        })]
    }
    fn type_steps(&self, text: &str, pid: Option<i32>) -> Vec<Step> {
        let (st, posted, text) = (self.0.clone(), self.1.clone(), text.to_string());
        vec![Step::new("type", move || {
            posted.lock().unwrap().push(format!(
                "type {} {}",
                repr(&text),
                if pid.is_some() {
                    "to pid"
                } else {
                    "foreground"
                }
            ));
            let mut s = st.lock().unwrap();
            if pid.is_none() || s.takes_background {
                s.value.push_str(&text);
            }
            Ok(())
        })]
    }
    fn key_steps(&self, combo: &str, pid: Option<i32>) -> Vec<Step> {
        let (st, posted, combo) = (self.0.clone(), self.1.clone(), combo.to_string());
        vec![Step::new(combo.clone(), move || {
            posted.lock().unwrap().push(format!(
                "key {combo} {}",
                if pid.is_some() {
                    "to pid"
                } else {
                    "foreground"
                }
            ));
            let mut s = st.lock().unwrap();
            if pid.is_none() || s.takes_background {
                s.text.push_str(&format!("[{combo}]"));
            }
            Ok(())
        })]
    }
    fn wheel_steps(
        &self,
        x: f64,
        y: f64,
        lines: i32,
        pid: Option<i32>,
        _h: bool,
        _w: Option<u32>,
        _r: bool,
        _p: bool,
        _o: (f64, f64),
    ) -> Vec<Step> {
        let posted = self.1.clone();
        vec![Step::new("scroll", move || {
            posted.lock().unwrap().push(format!(
                "scroll {lines} at {},{}{}",
                pyf(x),
                pyf(y),
                if pid.is_some() { " to pid" } else { "" }
            ));
            Ok(())
        })]
    }
    fn pointer_steps(
        &self,
        _s: (f64, f64),
        _e: (f64, f64),
        _k: &str,
        _pid: i32,
        _w: Option<u32>,
        _r: bool,
        _o: (f64, f64),
    ) -> Result<(Vec<Step>, Cleanup)> {
        Ok((vec![], Arc::new(|| {})))
    }
}

// ---------- the capture fakes (mackit.VisionAx, FakeShots, FakeApps) ----------

const BOUNDS: Rect = (100.0, 50.0, 800.0, 600.0);
const PAGE_AREA: Rect = (100.0, 130.0, 800.0, 520.0);

#[derive(Default)]
struct VisionState {
    secure: Vec<Option<bool>>,
    clicks: i64,
    secret_rects: Vec<Rect>,
    calls: Vec<String>,
    captured: Vec<i64>,
    raised: i64,
}

struct VisionAx(Arc<Mutex<VisionState>>);

impl Ax for VisionAx {
    fn read_tree(
        &self,
        _pid: i32,
        _b: bool,
        _w: Option<&El>,
        _o: usize,
        _bs: Option<f64>,
        _m: bool,
    ) -> AxTree {
        let mut s = self.0.lock().unwrap();
        let secure = if s.secure.is_empty() {
            Some(false)
        } else {
            s.secure.remove(0)
        };
        AxTree {
            text: format!(" clicks={}", s.clicks),
            secure,
            has_web: true,
            web_areas: vec![PAGE_AREA],
            secret_rects: Some(s.secret_rects.clone()),
            window: Some(el("win")),
            ..Default::default()
        }
    }
    fn expose_tree(&self, _pid: i32) -> bool {
        self.0.lock().unwrap().calls.push("ax.expose".into());
        false
    }
    fn app_window(&self, _pid: i32) -> Option<El> {
        Some(el("win"))
    }
    fn frame(&self, _e: &El) -> Option<Rect> {
        Some(BOUNDS)
    }
    fn window_with_frame(
        &self,
        _pid: i32,
        _b: Rect,
        _t: Option<&str>,
        _c: Option<&dyn Fn() -> String>,
    ) -> Option<El> {
        Some(el("win"))
    }
    fn element_window(&self, _e: &El) -> Option<El> {
        Some(el("win"))
    }
    fn window_title(&self, _w: &El) -> String {
        "Window".into()
    }
    fn web_areas_of(&self, _w: &El) -> Option<Vec<Rect>> {
        Some(vec![PAGE_AREA])
    }
    fn has_focus(&self, _pid: i32, _e: &El) -> Option<bool> {
        None
    }
    fn value(&self, _e: &El) -> Option<String> {
        None
    }
    fn press(&self, _e: &El) -> Result<bool> {
        self.0.lock().unwrap().calls.push("ax.press".into());
        Ok(true)
    }
    fn set_value(&self, _e: &El, _t: &str) -> Result<bool> {
        self.0.lock().unwrap().calls.push("ax.set_value".into());
        Ok(true)
    }
    fn focus(&self, _e: &El) -> bool {
        self.0.lock().unwrap().calls.push("ax.focus".into());
        true
    }
    fn center(&self, _n: &AxNode) -> (f64, f64) {
        (10.0, 10.0)
    }
    fn menu_item_for_shortcut(
        &self,
        _pid: i32,
        _k: &str,
        _m: &[String],
    ) -> Option<(El, String, bool)> {
        None
    }
    fn raise_window(&self, _pid: i32, _b: Rect) -> bool {
        self.0.lock().unwrap().raised += 1;
        true
    }
}

struct FakeShots(Arc<Mutex<VisionState>>);

impl Shots for FakeShots {
    fn window(&self, wid: i64) -> Option<Window> {
        Some(Window::new(wid, (wid / 10) as i32, BOUNDS, "w"))
    }
    fn front_window(&self, pid: i32, _like: Option<Rect>, _t: Option<&str>) -> Option<Window> {
        Some(Window::new(pid as i64 * 10, pid, BOUNDS, "w"))
    }
    fn on_screen(&self, _id: i64) -> bool {
        true
    }
    fn window_at(&self, _x: f64, _y: f64) -> Option<Window> {
        Some(Window::new(110, 0, BOUNDS, "w"))
    }
    fn windows_at(&self, x: f64, y: f64) -> Option<Vec<Window>> {
        Some(self.window_at(x, y).into_iter().collect())
    }
    fn capture(&self, w: &Window, _m: Option<u32>, _p: bool) -> Result<(Vec<u8>, i64, i64)> {
        self.0.lock().unwrap().captured.push(w.id);
        Ok((b"\xff\xd8\xffJPEG".to_vec(), 1600, 1200))
    }
}

struct FakeApps(Mutex<Vec<RunningApp>>);

impl Apps for FakeApps {
    fn find(&self, q: &str) -> Option<RunningApp> {
        find_in(&self.0.lock().unwrap(), q)
    }
    fn running(&self) -> Vec<RunningApp> {
        self.0.lock().unwrap().clone()
    }
    fn pids_of(&self, bundle_ids: &[&str]) -> Vec<i32> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|a| bundle_ids.contains(&a.bundle_id.to_lowercase().as_str()))
            .map(|a| a.pid)
            .collect()
    }
    fn frontmost(&self) -> Option<RunningApp> {
        self.0.lock().unwrap().iter().find(|a| a.active).cloned()
    }
    fn activate(&self, bid: &str, _pid: Option<i32>) -> bool {
        let mut apps = self.0.lock().unwrap();
        for a in apps.iter_mut() {
            a.active = a.bundle_id == bid;
        }
        true
    }
}

struct VisionGestures(Arc<Mutex<VisionState>>, Arc<Mutex<Vec<String>>>);

impl VisionGestures {
    fn clicks(&self, x: f64, y: f64, pid: Option<i32>) -> Vec<Step> {
        let where_ = if pid.is_some() { " to pid" } else { "" };
        let (p1, p2, st) = (self.1.clone(), self.1.clone(), self.0.clone());
        let (m, c) = (
            format!("move {},{}{where_}", pyf(x), pyf(y)),
            format!("click {},{}{where_}", pyf(x), pyf(y)),
        );
        vec![
            Step::new("move", move || {
                p1.lock().unwrap().push(m.clone());
                Ok(())
            }),
            Step::new("click", move || {
                p2.lock().unwrap().push(c.clone());
                if pid.is_some() {
                    st.lock().unwrap().clicks += 1;
                }
                Ok(())
            }),
        ]
    }
}

impl Gestures for VisionGestures {
    fn click_steps(&self, x: f64, y: f64, pid: Option<i32>) -> Vec<Step> {
        self.clicks(x, y, pid)
    }
    fn type_steps(&self, _t: &str, _pid: Option<i32>) -> Vec<Step> {
        vec![]
    }
    fn key_steps(&self, _c: &str, _pid: Option<i32>) -> Vec<Step> {
        vec![]
    }
    fn wheel_steps(
        &self,
        x: f64,
        y: f64,
        lines: i32,
        pid: Option<i32>,
        _h: bool,
        _w: Option<u32>,
        _r: bool,
        _p: bool,
        _o: (f64, f64),
    ) -> Vec<Step> {
        let posted = self.1.clone();
        vec![Step::new("scroll", move || {
            posted.lock().unwrap().push(format!(
                "scroll {lines} at {},{}{}",
                pyf(x),
                pyf(y),
                if pid.is_some() { " to pid" } else { "" }
            ));
            Ok(())
        })]
    }
    fn pointer_steps(
        &self,
        s: (f64, f64),
        _e: (f64, f64),
        _k: &str,
        pid: i32,
        _w: Option<u32>,
        _r: bool,
        _o: (f64, f64),
    ) -> Result<(Vec<Step>, Cleanup)> {
        Ok((self.clicks(s.0, s.1, Some(pid)), Arc::new(|| {})))
    }
}

// ---------- shared seams ----------

struct NoShots;
impl Shots for NoShots {
    fn window(&self, _id: i64) -> Option<Window> {
        None
    }
    fn front_window(&self, _p: i32, _l: Option<Rect>, _t: Option<&str>) -> Option<Window> {
        None
    }
    fn on_screen(&self, _id: i64) -> bool {
        false
    }
    fn window_at(&self, _x: f64, _y: f64) -> Option<Window> {
        None
    }
    fn capture(&self, _w: &Window, _m: Option<u32>, _p: bool) -> Result<(Vec<u8>, i64, i64)> {
        Err(HandError::Other("no capture".into()))
    }
}

/// The policy module's answers as Python's gave them (driver.json `oracle`).
struct Oracle(Map<String, Value>);

impl Oracle {
    fn get(&self, args: Value) -> Value {
        let k = ferb_py::json::dumps(&args);
        self.0
            .get(&k)
            .cloned()
            .unwrap_or_else(|| panic!("a policy question Python never asked: {k}"))
    }
}

impl Policy for Oracle {
    fn surface(
        &self,
        b: &str,
        chain: &[Facts],
        owner: &[String],
        shell: bool,
        unread: bool,
    ) -> Option<String> {
        let five = json!(["surface", b, chain, owner, shell, unread]);
        let k5 = ferb_py::json::dumps(&five);
        let v = if self.0.contains_key(&k5) || unread {
            self.get(five)
        } else {
            self.get(json!(["surface", b, chain, owner, shell]))
        };
        v.as_str().map(String::from)
    }
    fn can_host(&self, b: &str, owner: &[String], shell: bool) -> bool {
        self.get(json!(["can_host", b, owner, shell]))
            .as_bool()
            .unwrap()
    }
    fn run_menu(&self, path: &[String]) -> bool {
        self.get(json!(["run_menu", path])).as_bool().unwrap()
    }
    fn terminal_menu(&self, title: &str) -> bool {
        self.get(json!(["terminal_menu", title])).as_bool().unwrap()
    }
    fn run_control(&self, chain: &[Facts], editor: bool) -> Option<String> {
        self.get(json!(["run_control", chain, editor]))
            .as_str()
            .map(String::from)
    }
    fn editor_app(&self, b: &str) -> bool {
        self.get(json!(["editor_app", b])).as_bool().unwrap()
    }
    fn structural(&self, f: &Facts) -> bool {
        self.get(json!(["structural", f])).as_bool().unwrap()
    }
    fn newline(&self, t: &str) -> bool {
        self.get(json!(["newline", t])).as_bool().unwrap()
    }
    fn code_runner(&self, b: &str) -> Option<String> {
        self.get(json!(["code_runner", b]))
            .as_str()
            .map(String::from)
    }
    fn opens_files(&self, b: &str) -> bool {
        self.get(json!(["opens_files", b])).as_bool().unwrap()
    }
    fn unasked(&self, why: &str, app: &str) -> String {
        self.get(json!(["unasked", why, app]))
            .as_str()
            .unwrap()
            .into()
    }
    fn browser_page(&self, a: Option<&str>) -> bool {
        self.get(json!(["browser_page", a])).as_bool().unwrap()
    }
    fn typed_destination(&self, t: &str) -> Option<String> {
        self.get(json!(["typed_destination", t]))
            .as_str()
            .map(String::from)
    }
}

/// Time that moves only when the driver sleeps.
#[derive(Default)]
struct FakeTime(Mutex<f64>);
impl Time for FakeTime {
    fn monotonic(&self) -> f64 {
        *self.0.lock().unwrap()
    }
    fn sleep(&self, s: f64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        *self.0.lock().unwrap() += s;
        Box::pin(async {})
    }
}

fn input(owner: Vec<bool>, poll_s: f64, yield_after_s: f64) -> RealInput {
    let owner = Arc::new(Mutex::new(owner));
    let mut ri = RealInput::new(
        Arc::new(move || {
            let mut o = owner.lock().unwrap();
            if o.is_empty() {
                false
            } else {
                o.remove(0)
            }
        }),
        GuiLock::new(),
    );
    ri.sleep = Arc::new(|_| Box::pin(async {}));
    ri.poll_s = poll_s;
    ri.yield_after_s = yield_after_s;
    ri
}

fn fixed_profiles() -> Arc<Profiles> {
    Arc::new(Profiles::new(
        None,
        Arc::new(|| "2026-01-01T00:00:00+00:00".to_string()),
    ))
}

struct Env {
    driver: Arc<MacDriver>,
    posted: Arc<Mutex<Vec<String>>>,
    chat: Option<Arc<Mutex<ChatState>>>,
    vision: Option<Arc<Mutex<VisionState>>>,
    painted: Arc<Mutex<Vec<Value>>>,
}

fn chat_env(fake: &Value, oracle: &Map<String, Value>) -> Env {
    let cfg = &fake["cfg"];
    let st = Arc::new(Mutex::new(ChatState {
        takes_background: cfg
            .get("takes_background")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        box_has_focus: Some(
            cfg.get("box_has_focus")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        press_ok: cfg.get("press_ok").and_then(Value::as_bool).unwrap_or(true),
        send_button: cfg
            .get("send_button")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        facts: fake["kind"] == "facts",
        focus_el: "box".into(),
        chains: fake
            .get("chains")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_array()
                                .unwrap()
                                .iter()
                                .map(|f| f.as_object().unwrap().clone())
                                .collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        ..Default::default()
    }));
    let terminal = fake.get("chains").is_some();
    let posted = Arc::new(Mutex::new(vec![]));
    let owner: Vec<bool> = fake
        .get("owner")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|b| b.as_bool().unwrap()).collect())
        .unwrap_or_default();
    let apps = ChatApps(Mutex::new(vec![
        RunningApp::new("net.whatsapp.WhatsApp", "WhatsApp", 21, false),
        RunningApp::new("com.apple.TextEdit", "TextEdit", 22, false),
        RunningApp::new("com.microsoft.VSCode", "Code", 23, true),
    ]));
    let mut d = MacDriver::new(Parts {
        device_id: "dev".into(),
        input: input(owner, 0.1, 5.0),
        gestures: Arc::new(ChatGestures(st.clone(), posted.clone())),
        ax: Arc::new(ChatAx(st.clone())),
        apps: Arc::new(apps),
        shots: Arc::new(NoShots),
        policy: Arc::new(Oracle(oracle.clone())),
        time: Arc::new(FakeTime::default()),
    });
    d.profiles = fixed_profiles();
    d.own_pid = 999_999;
    if terminal {
        d.clipboard = Arc::new(|| (7, Some("codex".to_string()), false));
    }
    Env {
        driver: Arc::new(d),
        posted,
        chat: Some(st),
        vision: None,
        painted: Arc::new(Mutex::new(vec![])),
    }
}

fn vision_env(fake: &Value, oracle: &Map<String, Value>) -> Env {
    let rects: Vec<Rect> = fake["cfg"]["secret_rects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r[0].as_f64().unwrap(),
                r[1].as_f64().unwrap(),
                r[2].as_f64().unwrap(),
                r[3].as_f64().unwrap(),
            )
        })
        .collect();
    let st = Arc::new(Mutex::new(VisionState {
        secret_rects: rects,
        ..Default::default()
    }));
    let posted = Arc::new(Mutex::new(vec![]));
    let apps = FakeApps(Mutex::new(vec![
        RunningApp::new("com.apple.Notes", "Notes", 11, false),
        RunningApp::new("com.brave.Browser", "Brave", 12, true),
        RunningApp::new("com.secret.Vault", "Vault", 13, false),
        RunningApp::new("com.apple.Passwords", "Passwords", 14, false),
    ]));
    let mut d = MacDriver::new(Parts {
        device_id: "dev".into(),
        input: input(vec![], 0.1, 0.2),
        gestures: Arc::new(VisionGestures(st.clone(), posted.clone())),
        ax: Arc::new(VisionAx(st.clone())),
        apps: Arc::new(apps),
        shots: Arc::new(FakeShots(st.clone())),
        policy: Arc::new(Oracle(oracle.clone())),
        time: Arc::new(FakeTime::default()),
    });
    d.profiles = fixed_profiles();
    d.own_pid = 999_999;
    d.excluded = Arc::new(|| {
        [
            "com.1password.1password",
            "com.agilebits.onepassword7",
            "com.bitwarden.desktop",
            "com.apple.Passwords",
            "com.apple.keychainaccess",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    });
    Env {
        driver: Arc::new(d),
        posted,
        chat: None,
        vision: Some(st),
        painted: Arc::new(Mutex::new(vec![])),
    }
}

fn action_of(v: &Value) -> ComputerAction {
    let kind = match v["kind"].as_str().unwrap() {
        "press" => K::Press,
        "set_value" => K::SetValue,
        "focus" => K::Focus,
        "click" => K::Click,
        "type" => K::Type,
        "key" => K::Key,
        "edit" => K::Edit,
        "scroll" => K::Scroll,
        "select" => K::Select,
        _ => K::Cancel,
    };
    ComputerAction {
        kind,
        element: v["element"].as_str().map(String::from),
        text: v["text"].as_str().map(String::from),
        select: v["select"].clone(),
        direction: v["direction"].as_str().map(String::from),
        amount: v["amount"].as_i64(),
        window: v["window"].as_str().map(String::from),
        allow_foreground: v["allow_foreground"].as_bool().unwrap_or(false),
        mechanism: v["mechanism"].as_str().map(|m| match m {
            "ax" => Mechanism::Ax,
            "pid" => Mechanism::Pid,
            "ax_value" => Mechanism::AxValue,
            _ => Mechanism::Foreground,
        }),
    }
}

fn ar_json(r: &ferb_core::types::ActionResult) -> Value {
    json!({"ok": r.ok, "detail": r.detail, "facts": r.facts, "observed": r.observed.as_ref().map(|f| Value::Object(f.recorded()))})
}

fn out_of<T>(r: Result<T>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!({"ok": f(v)}),
        Err(e) => json!({"error": e.class(), "message": e.to_string()}),
    }
}

fn ctx(scopes: &[&str]) -> ToolContext {
    ToolContext {
        task_id: "t1.worker".into(),
        worker_id: "w1".into(),
        request_id: "r1".into(),
        identity: Identity::Ferb,
        scopes: scopes
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

async fn run_step(env: &Env, tools: &MacTools, op: &str, args: &Value) -> Value {
    let d = &env.driver;
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(String::from);
    let f = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    match op {
        "snapshot" => out_of(
            d.snapshot(&s("app").unwrap(), None, None, 0).await,
            |(_, o)| Value::Object(o),
        ),
        "act" => {
            let a = action_of(&args["action"]);
            let app = s("app").unwrap_or_else(|| "WhatsApp".into());
            let grant: Option<TerminalGrant> = match args.get("grant") {
                Some(g) if !g.is_null() => {
                    Some(serde_json::from_value(g.clone()).expect("a grant"))
                }
                _ => None,
            };
            out_of(
                d.act(&app, &a, "w1", Some("t1.worker"), grant.as_ref())
                    .await,
                |r| ar_json(&r),
            )
        }
        "secret_terminal_input" => out_of(
            d.secret_terminal_input("Code", "s1e1", "the server password"),
            |t| json!(t),
        ),
        "menu_terminal_input" => out_of(
            d.menu_terminal_input("Code", &["Edit".to_string(), "Paste".to_string()]),
            |t| json!(t),
        ),
        "press_menu" => out_of(
            d.press_menu(
                "Code",
                &["Edit".to_string(), "Paste".to_string()],
                None,
                Some("t1.worker"),
            )
            .await,
            |t| json!(t),
        ),
        "element_identity" => {
            json!({"ok": d.element_identity("net.whatsapp.WhatsApp", &s("element").unwrap())})
        }
        "app" => {
            let q = s("q").unwrap();
            if q == "Passwords" {
                out_of(d.app(&q), |a| json!(a.name))
            } else {
                out_of(d.app(&q), |a| {
                    json!(format!(
                        "RunningApp(bundle_id={}, name={}, pid={}, active={})",
                        repr(&a.bundle_id),
                        repr(&a.name),
                        a.pid,
                        if a.active { "True" } else { "False" }
                    ))
                })
            }
        }
        "set_focus" => {
            env.chat.as_ref().unwrap().lock().unwrap().focus_el = args.as_str().unwrap().into();
            json!({"ok": null})
        }
        "terminal_input" => {
            let app = s("app").unwrap_or_else(|| "WhatsApp".into());
            out_of(
                d.terminal_input(&app, &action_of(&args["action"]), Some("t1.worker")),
                |t| json!(t),
            )
        }
        "browser_page" => out_of(
            d.browser_page("WhatsApp", &action_of(&args["action"])),
            |t| json!(t),
        ),
        "screenshot" => {
            if args.get("browser").is_some() {
                let paint = |j: Vec<u8>, _m: &[Rect], _b: Rect, _s: (i64, i64)| Ok(j);
                return out_of(
                    d.screenshot("Brave", None, false, &paint).await,
                    |(_, s, _)| json!(s.masking),
                );
            }
            if let Some(sec) = args.get("secure").and_then(Value::as_array) {
                env.vision
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .secure
                    .extend(sec.iter().map(Value::as_bool));
            }
            let painted = env.painted.clone();
            let paint = move |j: Vec<u8>, m: &[Rect], b: Rect, sz: (i64, i64)| {
                let boxes = ferb_mac_hands::mask::to_image_boxes(m, b, sz);
                painted.lock().unwrap().push(json!(boxes
                    .iter()
                    .map(|x| json!([x.0, x.1, x.2, x.3]))
                    .collect::<Vec<_>>()));
                Ok(if boxes.is_empty() {
                    j
                } else {
                    b"painted".to_vec()
                })
            };
            let got = d.screenshot("Notes", None, false, &paint).await;
            let st = env.vision.as_ref().unwrap().lock().unwrap();
            let last_painted = env.painted.lock().unwrap().last().cloned();
            out_of(got, |(a, sh, jpeg)| {
                json!({"app": a.name, "shot": sh.id, "window": sh.window, "bounds": [sh.bounds.0, sh.bounds.1, sh.bounds.2, sh.bounds.3],
                       "size": [sh.size.0, sh.size.1], "pid": sh.pid, "masking": sh.masking,
                       "masks": sh.masks.iter().map(|m| json!([m.0, m.1, m.2, m.3])).collect::<Vec<_>>(),
                       "jpeg": jpeg.iter().map(|b| *b as char).collect::<String>(), "painted": last_painted, "captured": st.captured})
            })
        }
        "click_identity" => {
            json!({"ok": d.click_identity("com.apple.Notes", "shot1", f("x"), f("y"))})
        }
        "shot_point" => out_of(
            d.shot_point("Notes", "shot1", f("x"), f("y")),
            |p| match p {
                Some((pid, w, (x, y))) => json!([pid, w, [x, y]]),
                None => Value::Null,
            },
        ),
        "click_at" => {
            let shot = s("shot").unwrap_or_else(|| {
                d.latest_shot("com.apple.Notes")
                    .map(|s| s.id)
                    .unwrap_or_default()
            });
            let allow = args["allow_foreground"].as_bool().unwrap_or(false);
            out_of(
                d.click_at(
                    "Notes",
                    &shot,
                    f("x"),
                    f("y"),
                    "w1",
                    Some("t1.worker"),
                    allow,
                    &s("mechanism").unwrap(),
                    None,
                )
                .await,
                |r| ar_json(&r),
            )
        }
        "pointer" => {
            let kind = s("kind").unwrap();
            let shot = if kind == "zoom" {
                "shot9".to_string()
            } else {
                d.latest_shot("com.apple.Notes")
                    .map(|s| s.id)
                    .unwrap_or_default()
            };
            let (x, y) = if kind == "zoom" {
                (1.0, 1.0)
            } else {
                (10.0, 10.0)
            };
            let end = if kind == "drag" {
                Some((100.0, 120.0))
            } else {
                None
            };
            let (dir, amount, dist) = if kind == "scroll" {
                ("down", 3, 0.5)
            } else {
                ("down", 1, 0.8)
            };
            let task = if kind == "hover" {
                Some("t1.worker")
            } else {
                None
            };
            out_of(
                d.pointer_at(
                    "Notes", &shot, x, y, &kind, end, "w1", dir, amount, dist, None, task,
                )
                .await,
                |r| ar_json(&r),
            )
        }
        "own_point" => json!({"ok": d.own_point(f("x"), f("y"), Some(11), None)}),
        op if op.starts_with("run ") => {
            let name = &op[4..];
            let c = ctx(&["mac:act", "mac:read"]);
            out_of(tools.run(name, args, &c).await, |t| {
                json!({"content": t.content, "is_error": t.is_error, "read_sources": t.read_sources.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
                       "facts": t.facts.as_ref().map(|f| Value::Object(f.recorded())), "image": t.image.is_some()})
            })
        }
        op if op.starts_with("describe ") => {
            let spec = tools.describe(&op[9..], args);
            json!({"ok": {"kind": spec.kind, "site": spec.target.site, "recipients": spec.target.recipients, "path": spec.target.path,
                          "contact": spec.target.contact, "content": spec.content, "declared": spec.declared, "place": spec.place}})
        }
        "precheck" => {
            let tool = s("tool").unwrap();
            let a = match (tool.as_str(), args.get("shell").is_some()) {
                ("mac.applescript", false) => {
                    json!({"app": "com.apple.systemevents", "script": "tell process \"FERB\""})
                }
                ("mac.applescript", true) => {
                    json!({"app": "WhatsApp", "script": "do shell script \"x\""})
                }
                ("mac.secret", _) => json!({"app": "Arc", "element": "s1e1"}),
                _ => json!({"app": "WhatsApp", "shot": "s", "x": 1, "y": 1, "action": "drag"}),
            };
            json!({"ok": tools.precheck(&tool, &a)})
        }
        "locks" => {
            json!({"ok": tools.locks("mac.act", &json!({"app": "WhatsApp", "action": "press"}))})
        }
        "ledger_args" => {
            let hashed =
                json!({"app": "net.whatsapp.WhatsApp", "action": "press", "element": "s1e2"});
            json!({"ok": tools.ledger_args("mac.act", &json!({"app": "WhatsApp", "action": "press", "element": "s1e2"}), hashed.as_object().unwrap())})
        }
        "menu_path" => {
            let mut a = json!({"app": "x", "path": s("path")});
            ferb_mac_hands::tools::normalize_args("mac.menu", &mut a).unwrap();
            json!({"ok": a["path"]})
        }
        "preview_target" => {
            let p = if args.get("observed").is_some() {
                let obs = json!({"windows": [{"id": 1}]});
                tools.preview_target("mac.snapshot", &json!({"app": "WhatsApp"}), obs.as_object())
            } else if s("action").as_deref() == Some("key") {
                tools.preview_target(
                    "mac.act",
                    &json!({"app": "WhatsApp", "action": "key", "element": "s1e1", "text": "a"}),
                    None,
                )
            } else {
                tools.preview_target(
                    "mac.act",
                    &json!({"app": "WhatsApp", "action": "press", "element": "s1e1"}),
                    None,
                )
            };
            json!({"ok": p.map(|p| json!([p.scope, p.label, p.resolve.is_some()]))})
        }
        other => panic!("unknown step {other}"),
    }
}

// decision: HAND-200, HAND-218, HAND-220, HAND-221, HAND-518, HAND-519
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_and_tools_do_what_python_did() {
    let fx = common::load("driver.json");
    let oracle = fx["oracle"].as_object().unwrap().clone();
    let (mut compared, mut skipped) = (0, 0);
    for sc in fx["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap();
        let fake = &sc["fake"];
        let env = if fake["kind"] == "vision" {
            vision_env(fake, &oracle)
        } else {
            chat_env(fake, &oracle)
        };
        let paint: ferb_mac_hands::driver::PaintFn = Arc::new(|j, _m, _b, _s| Ok(j));
        let tools = MacTools::new(env.driver.clone(), paint);
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let op = step["op"].as_str().unwrap();
            if step["out"].get("error") == Some(&json!("AttributeError")) {
                skipped += 1;
                continue;
            }
            let posted_before = env.posted.lock().unwrap().len();
            let calls_before = env
                .chat
                .as_ref()
                .map(|c| c.lock().unwrap().calls.len())
                .or(env.vision.as_ref().map(|v| v.lock().unwrap().calls.len()))
                .unwrap();
            let got = run_step(&env, &tools, op, &step["args"]).await;
            assert_eq!(got, step["out"], "{name} step {i} ({op})");
            let posted: Vec<String> = env.posted.lock().unwrap()[posted_before..].to_vec();
            assert_eq!(
                json!(posted),
                step["posted"],
                "{name} step {i} ({op}): posted"
            );
            let calls: Vec<String> = match (&env.chat, &env.vision) {
                (Some(c), _) => c.lock().unwrap().calls[calls_before..].to_vec(),
                (_, Some(v)) => v.lock().unwrap().calls[calls_before..].to_vec(),
                _ => vec![],
            };
            assert_eq!(json!(calls), step["calls"], "{name} step {i} ({op}): calls");
            compared += 1;
        }
    }
    assert!(compared > 90, "{compared} compared, {skipped} skipped");
    let _ = HashMap::<(), ()>::new();
}

#[test]
fn declarations_match_the_catalog() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/catalog/tools.json");
    let cat: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let tools: HashMap<String, Value> = cat["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t.clone()))
        .collect();
    for d in ferb_mac_hands::tools::declarations() {
        let t = &tools[d.name];
        assert_eq!(
            t["description"].as_str().unwrap(),
            d.description,
            "{}",
            d.name
        );
        assert_eq!(
            t["scopes"],
            json!(d.scopes.iter().copied().collect::<BTreeSet<_>>()),
            "{}",
            d.name
        );
        assert_eq!(
            t["identities"],
            json!(ferb_mac_hands::tools::identities()),
            "{}",
            d.name
        );
        assert_eq!(t["facts"], d.facts, "{}", d.name);
    }
}
