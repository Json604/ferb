//! An element's own Accessibility actions (HAND-551, HAND-552), an app that brings itself to the front on FERB's input
//! (HAND-549, HAND-553, HAND-554), on fakes: a chat app's window whose document bubble advertises AXShowMenu, a
//! message with the app's own named actions, a context menu that opens when the bubble is asked for it, and the owner's
//! notes app in front. Nothing here reads or changes the real Mac; the by-hand check on a real app is the ignored test
//! at the end.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{FakeTree, Src};
use ferb_core::types::{ComputerAction, ComputerActionKind as K, Identity, Mechanism, ToolContext};
use ferb_mac_hands::apps::RunningApp;
use ferb_mac_hands::ax::{
    self, AxNode, AxSource, AxTree, AxValue, CgWindow, El, Rect, ScrollPosition,
};
use ferb_mac_hands::driver::{
    Apps, Ax, Facts, MacDriver, Parts, Policy, Shots, Time, Window, OWN_APP,
};
use ferb_mac_hands::error::{HandError, Result};
use ferb_mac_hands::input::{Cleanup, Gestures, RealInput, Step};
use ferb_mac_hands::locks::GuiLock;
use ferb_mac_hands::profile::RAW_INPUT_BACKGROUND;
use ferb_mac_hands::tools::MacTools;
use serde_json::{json, Value};

const CHAT: i32 = 50;
const NOTES: i32 = 60;
const OTHER: i32 = 70;
const NOTCH: i32 = 10;
const CHAT_ID: &str = "com.example.Chat";
const NOTES_ID: &str = "com.example.Notes";
const WIN: Rect = (0.0, 0.0, 800.0, 600.0);

// ---------- the chat app's elements ----------

fn spec() -> Value {
    let at = |x: f64, y: f64, w: f64, h: f64| json!({"AXPosition": {"pt": [x, y]}, "AXSize": {"sz": [w, h]}});
    let mut els = serde_json::Map::new();
    let mut add = |id: &str, mut attrs: Value, extra: Value, actions: Vec<&str>| {
        for (k, v) in extra.as_object().unwrap() {
            attrs[k] = v.clone();
        }
        els.insert(id.into(), json!({"attrs": attrs, "actions": actions}));
    };
    add(
        "app",
        json!({}),
        json!({"AXRole": "AXApplication", "AXChildren": {"els": ["win"]}, "AXWindows": {"els": ["win"]},
               "AXFocusedWindow": {"el": "win"}, "AXMainWindow": {"el": "win"}, "AXFocusedUIElement": {"el": "send"}}),
        vec![],
    );
    add(
        "win",
        at(0.0, 0.0, 800.0, 600.0),
        json!({"AXRole": "AXWindow", "AXTitle": "Chat", "AXChildren": {"els": ["doc", "send", "msg"]}}),
        vec![],
    );
    let inside = json!({"AXWindow": {"el": "win"}, "AXParent": {"el": "win"}});
    add(
        "doc",
        at(10.0, 10.0, 200.0, 40.0),
        json!({"AXRole": "AXGroup", "AXDescription": "Document bubble", "AXWindow": {"el": "win"},
               "AXParent": {"el": "win"}}),
        vec!["AXPress", "AXScrollToVisible", "AXCancel", "AXShowMenu"],
    );
    let mut send = inside.clone();
    send["AXRole"] = json!("AXButton");
    send["AXTitle"] = json!("Send");
    add("send", at(10.0, 500.0, 60.0, 30.0), send, vec!["AXPress"]);
    let mut msg = inside;
    msg["AXRole"] = json!("AXGroup");
    msg["AXDescription"] = json!("Message");
    add(
        "msg",
        at(10.0, 100.0, 400.0, 40.0),
        msg,
        vec![
            "AXPress",
            "Name:Save As\u{2026}\nTarget:0x0\nSelector:(null)",
            "Name:Download\nTarget:0x0\nSelector:(null)",
            "AXShowAlternateUI",
        ],
    );
    add(
        "ctx",
        at(200.0, 50.0, 180.0, 90.0),
        json!({"AXRole": "AXMenu", "AXChildren": {"els": ["mi_save", "mi_delete", "mi_quit"]},
               "AXParent": {"el": "app"}}),
        vec!["AXCancel"],
    );
    let item = |t: &str| json!({"AXRole": "AXMenuItem", "AXTitle": t, "AXParent": {"el": "ctx"}, "AXEnabled": true});
    add(
        "mi_save",
        at(200.0, 50.0, 180.0, 30.0),
        item("Save As\u{2026}"),
        vec!["AXPress", "AXCancel"],
    );
    add(
        "mi_delete",
        at(200.0, 80.0, 180.0, 30.0),
        item("Delete"),
        vec!["AXPress", "AXCancel"],
    );
    let mut quit = item("Quit Chat");
    quit["AXMenuItemCmdChar"] = json!("Q");
    quit["AXMenuItemCmdModifiers"] = json!(0);
    add(
        "mi_quit",
        at(200.0, 110.0, 180.0, 30.0),
        quit,
        vec!["AXPress", "AXCancel"],
    );
    json!({"els": els, "apps": {"50": "app"},
           "cg": {"50": [{"id": 500, "title": "Chat", "onscreen": true, "bounds": [0.0, 0.0, 800.0, 600.0]}]}})
}

/// The chat app's Accessibility: the fake tree, with the bubble's context menu open (an AXMenu child of the app)
/// from its AXShowMenu until a cancel or an item's press; `raises` brings the app to the front when its menu opens.
struct ChatSrc {
    tree: Arc<FakeTree>,
    open: Mutex<bool>,
    apps: Arc<FakeApps>,
    raises: Arc<AtomicBool>,
}

impl AxSource for ChatSrc {
    fn system(&self) -> El {
        Src(self.tree.clone()).system()
    }
    fn app(&self, pid: i32) -> El {
        Src(self.tree.clone()).app(pid)
    }
    fn attr(&self, el: &El, name: &str) -> AxValue {
        if FakeTree::id_of(el) == "app" && name == "AXChildren" && *self.open.lock().unwrap() {
            return AxValue::List(vec![
                AxValue::El(self.tree.el("win")),
                AxValue::El(self.tree.el("ctx")),
            ]);
        }
        Src(self.tree.clone()).attr(el, name)
    }
    fn actions(&self, el: &El) -> Vec<String> {
        Src(self.tree.clone()).actions(el)
    }
    fn perform(&self, el: &El, action: &str) -> i32 {
        let id = FakeTree::id_of(el);
        match (id.as_str(), action) {
            ("doc", "AXShowMenu") => {
                *self.open.lock().unwrap() = true;
                if self.raises.load(Ordering::SeqCst) {
                    *self.apps.front.lock().unwrap() = CHAT;
                }
            }
            ("ctx", "AXCancel") => *self.open.lock().unwrap() = false,
            (i, "AXPress") if i.starts_with("mi_") => *self.open.lock().unwrap() = false,
            _ => {}
        }
        Src(self.tree.clone()).perform(el, action)
    }
    fn set(&self, el: &El, name: &str, value: AxValue) -> i32 {
        Src(self.tree.clone()).set(el, name, value)
    }
    fn element_at(&self, _root: &El, _x: f64, _y: f64) -> Option<El> {
        None
    }
    fn pid_of(&self, _el: &El) -> Option<i32> {
        Some(CHAT)
    }
    fn same(&self, a: &El, b: &El) -> bool {
        FakeTree::id_of(a) == FakeTree::id_of(b)
    }
    fn hash(&self, el: &El) -> u64 {
        Src(self.tree.clone()).hash(el)
    }
    fn cg_windows(&self, pid: i32) -> Vec<CgWindow> {
        Src(self.tree.clone()).cg_windows(pid)
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
    fn sleep(&self, _seconds: f64) {}
}

/// The driver's Accessibility over the real walk and element operations (`ax::Ax`), as the native one is.
struct ChatAx(ax::Ax<ChatSrc>);

fn ok(code: std::result::Result<bool, ax::NoAnswer>) -> Result<bool> {
    code.map_err(HandError::NoAnswer)
}

impl Ax for ChatAx {
    fn read_tree(
        &self,
        pid: i32,
        browser: bool,
        window: Option<&El>,
        offset: usize,
        budget_s: Option<f64>,
        masks_only: bool,
    ) -> AxTree {
        self.0
            .read_tree(pid, browser, window, offset, budget_s, masks_only)
    }
    fn expose_tree(&self, _pid: i32) -> bool {
        true
    }
    fn app_window(&self, pid: i32) -> Option<El> {
        self.0.app_window(pid)
    }
    fn ax_windows(&self, pid: i32) -> Option<Vec<El>> {
        Some(self.0.ax_windows(pid))
    }
    fn windows(&self, pid: i32) -> Vec<Facts> {
        self.0.windows(pid, None)
    }
    fn frame(&self, el: &El) -> Option<Rect> {
        self.0.frame(Some(el))
    }
    fn window_with_frame(
        &self,
        pid: i32,
        bounds: Rect,
        title: Option<&str>,
        current: Option<&dyn Fn() -> String>,
    ) -> Option<El> {
        self.0.window_with_frame(pid, bounds, title, current)
    }
    fn element_window(&self, el: &El) -> Option<El> {
        self.0.element_window(el)
    }
    fn parent_of(&self, el: &El) -> Option<El> {
        self.0.parent_of(el)
    }
    fn role_of(&self, el: &El) -> String {
        self.0.role_of(el)
    }
    fn subrole_of(&self, el: &El) -> String {
        self.0.subrole_of(el)
    }
    fn focused_element(&self, pid: i32) -> Option<El> {
        self.0.focused_element(pid)
    }
    fn same(&self, a: &El, b: &El) -> bool {
        self.0.same(Some(a), Some(b))
    }
    fn surface_facts(&self, el: &El) -> Option<Vec<Facts>> {
        Some(self.0.surface_facts(Some(el)))
    }
    fn window_facts(&self, el: &El) -> Option<Vec<Option<Facts>>> {
        Some(self.0.window_facts(el))
    }
    fn item_shortcut(&self, el: &El) -> Option<(String, i64)> {
        self.0.item_shortcut(el)
    }
    fn press(&self, el: &El) -> Result<bool> {
        ok(self.0.perform(Some(el), "AXPress"))
    }
    fn perform(&self, el: &El, action: &str) -> Result<bool> {
        ok(self.0.perform(Some(el), action))
    }
    fn cancel(&self, el: &El) -> Result<bool> {
        ok(self.0.perform(Some(el), "AXCancel"))
    }
    fn focus(&self, el: &El) -> bool {
        self.0.focus(el)
    }
    fn has_focus(&self, pid: i32, el: &El) -> Option<bool> {
        self.0.has_focus(pid, el)
    }
    fn set_value(&self, el: &El, text: &str) -> Result<bool> {
        ok(self.0.set_value(el, text))
    }
    fn value(&self, el: &El) -> Option<String> {
        self.0.value(el)
    }
    fn menu_item_for_shortcut(
        &self,
        pid: i32,
        key: &str,
        mods: &[String],
    ) -> Option<(El, String, bool)> {
        self.0.menu_item_for_shortcut(pid, key, mods)
    }
    fn scroll_position(&self, area: Option<&El>, content: Option<&El>) -> ScrollPosition {
        self.0.scroll_position(area, content)
    }
    fn popup_menu(&self, el: &El, pid: i32) -> Option<El> {
        self.0.popup_menu(el, pid)
    }
    fn menu_items(&self, menu: &El) -> Vec<(El, String)> {
        self.0.menu_items(menu)
    }
    fn action_list(&self, el: &El) -> Vec<String> {
        self.0.src.actions(el)
    }
    fn open_menu(&self, el: &El, pid: i32) -> Option<El> {
        self.0.context_menu(el, pid)
    }
}

// ---------- the Mac around it ----------

/// The running apps: the chat app, the owner's notes app (in front), another app and FERB's notch; every activation.
struct FakeApps {
    running: Vec<RunningApp>,
    front: Mutex<i32>,
    calls: Mutex<Vec<String>>,
}

impl FakeApps {
    fn new() -> Self {
        FakeApps {
            running: vec![
                RunningApp::new(CHAT_ID, "Chat", CHAT, false),
                RunningApp::new(NOTES_ID, "Notes", NOTES, true),
                RunningApp::new("com.example.Other", "Other", OTHER, false),
                RunningApp::new("dev.ferb.notch", "FERB Notch", NOTCH, false),
            ],
            front: Mutex::new(NOTES),
            calls: Mutex::new(vec![]),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn front(&self) -> i32 {
        *self.front.lock().unwrap()
    }
}

impl Apps for FakeApps {
    fn find(&self, q: &str) -> Option<RunningApp> {
        let m: Vec<&RunningApp> = self
            .running
            .iter()
            .filter(|a| {
                a.bundle_id.eq_ignore_ascii_case(q) || a.name == q || format!("pid:{}", a.pid) == q
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
                    .any(|b| b.eq_ignore_ascii_case(&a.bundle_id))
            })
            .map(|a| a.pid)
            .collect()
    }
    fn frontmost(&self) -> Option<RunningApp> {
        let f = self.front();
        self.running.iter().find(|a| a.pid == f).cloned()
    }
    fn activate(&self, bundle_id: &str, pid: Option<i32>) -> bool {
        self.calls
            .lock()
            .unwrap()
            .push(format!("activate {bundle_id}"));
        *self.front.lock().unwrap() = pid.unwrap_or(0);
        true
    }
}

/// The chat window in the window server's list, captured as an invented image.
struct ChatShots;

impl Shots for ChatShots {
    fn window(&self, id: i64) -> Option<Window> {
        (id == 500).then(|| Window::new(500, CHAT, WIN, "Chat"))
    }
    fn front_window(&self, pid: i32, _like: Option<Rect>, _title: Option<&str>) -> Option<Window> {
        (pid == CHAT).then(|| Window::new(500, CHAT, WIN, "Chat"))
    }
    fn on_screen(&self, _id: i64) -> bool {
        true
    }
    fn window_at(&self, _x: f64, _y: f64) -> Option<Window> {
        None
    }
    fn capture(&self, _w: &Window, _m: Option<u32>, _p: bool) -> Result<(Vec<u8>, i64, i64)> {
        Ok((b"\xff\xd8\xffJPEG".to_vec(), 800, 600))
    }
}

/// Pointer events recorded as they would be posted; with `raises`, the app comes to the front on the first one (a
/// mouse-down), as the chat app did on a right-click (2026-11-15).
struct Pointer {
    posted: Arc<Mutex<Vec<String>>>,
    apps: Arc<FakeApps>,
    raises: Arc<AtomicBool>,
}

impl Gestures for Pointer {
    fn click_steps(&self, _x: f64, _y: f64, _pid: Option<i32>) -> Vec<Step> {
        vec![]
    }
    fn type_steps(&self, t: &str, _pid: Option<i32>) -> Vec<Step> {
        let p = self.posted.clone();
        let t = t.to_string();
        vec![Step::new("type", move || {
            p.lock().unwrap().push(format!("type {t}"));
            Ok(())
        })]
    }
    fn key_steps(&self, _c: &str, _pid: Option<i32>) -> Vec<Step> {
        vec![]
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
        vec![]
    }
    fn pointer_steps(
        &self,
        _s: (f64, f64),
        _e: (f64, f64),
        kind: &str,
        _p: i32,
        _w: Option<u32>,
        _r: bool,
        _o: (f64, f64),
    ) -> Result<(Vec<Step>, Cleanup)> {
        let (p1, p2, apps, raises) = (
            self.posted.clone(),
            self.posted.clone(),
            self.apps.clone(),
            self.raises.clone(),
        );
        let (k1, k2) = (kind.to_string(), kind.to_string());
        Ok((
            vec![
                Step::new("down", move || {
                    p1.lock().unwrap().push(format!("{k1} down"));
                    if raises.load(Ordering::SeqCst) {
                        *apps.front.lock().unwrap() = CHAT;
                    }
                    Ok(())
                }),
                Step::new("up", move || {
                    p2.lock().unwrap().push(format!("{k2} up"));
                    Ok(())
                }),
            ],
            Arc::new(|| {}),
        ))
    }
}

/// No app here is a terminal; nothing runs code.
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

struct Mac {
    driver: Arc<MacDriver>,
    tools: MacTools,
    apps: Arc<FakeApps>,
    tree: Arc<FakeTree>,
    src_raises: Arc<ChatSrcHandle>,
    pointer_raises: Arc<AtomicBool>,
    owner_active: Arc<AtomicBool>,
    waited: Arc<Mutex<Vec<bool>>>,
    told: Arc<Mutex<Vec<(String, Value)>>>,
    posted: Arc<Mutex<Vec<String>>>,
}

/// Whether the chat app's menu brings it forward (the source is inside the driver).
struct ChatSrcHandle(Arc<AtomicBool>);

fn mac() -> Mac {
    let apps = Arc::new(FakeApps::new());
    let tree = Arc::new(FakeTree::from_spec(&spec(), 0.0));
    let raises = Arc::new(AtomicBool::new(false));
    let src = ChatSrc {
        tree: tree.clone(),
        open: Mutex::new(false),
        apps: apps.clone(),
        raises: raises.clone(),
    };
    let src_raises = raises.clone();
    let owner_active = Arc::new(AtomicBool::new(false));
    let oa = owner_active.clone();
    let mut input = RealInput::new(Arc::new(move || oa.load(Ordering::SeqCst)), GuiLock::new());
    input.sleep = Arc::new(|_| Box::pin(async {}));
    input.yield_after_s = 0.2;
    let waited = Arc::new(Mutex::new(vec![]));
    let w = waited.clone();
    input.on_wait = Arc::new(move |_, on| w.lock().unwrap().push(on));
    let posted = Arc::new(Mutex::new(vec![]));
    let pointer_raises = Arc::new(AtomicBool::new(false));
    let ax = ChatAx(ax::Ax::new(src));
    let mut d = MacDriver::new(Parts {
        device_id: "dev".into(),
        input,
        gestures: Arc::new(Pointer {
            posted: posted.clone(),
            apps: apps.clone(),
            raises: pointer_raises.clone(),
        }),
        ax: Arc::new(ax),
        apps: apps.clone(),
        shots: Arc::new(ChatShots),
        policy: Arc::new(Plain),
        time: Arc::new(PausedTime::default()),
    });
    d.own_pid = 999_999;
    let told = Arc::new(Mutex::new(vec![]));
    let t = told.clone();
    d.came_forward = Some(Arc::new(move |task: &str, shown: Value| {
        t.lock().unwrap().push((task.to_string(), shown));
    }));
    let driver = Arc::new(d);
    let tools = MacTools::new(driver.clone(), Arc::new(|j, _m, _b, _s| Ok(j)));
    Mac {
        driver,
        tools,
        apps,
        tree,
        src_raises: Arc::new(ChatSrcHandle(src_raises)),
        pointer_raises,
        owner_active,
        waited,
        told,
        posted,
    }
}

impl Mac {
    /// The chat source's own `raises` flag lives inside the driver; flip it there.
    fn menu_raises(&self) {
        self.src_raises.0.store(true, Ordering::SeqCst);
    }
    fn calls(&self) -> Vec<String> {
        self.tree.calls.lock().unwrap().clone()
    }
    async fn run(&self, tool: &str, args: Value) -> ferb_core::types::ToolResult {
        self.tools.run(tool, &args, &ctx("t1")).await.unwrap()
    }
    async fn snapshot(&self) -> Value {
        Value::Object(
            self.run("mac.snapshot", json!({"app": "Chat"}))
                .await
                .content,
        )
    }
    /// The id of the element labelled `label` in a snapshot or a menu's listed elements.
    fn id(elements: &Value, label: &str) -> String {
        elements
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["label"] == label)
            .unwrap_or_else(|| panic!("no {label} in {elements}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn ctx(task: &str) -> ToolContext {
    ToolContext {
        task_id: task.into(),
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

fn element(kind: K, element: &str, name: Option<&str>) -> ComputerAction {
    ComputerAction {
        kind,
        element: Some(element.into()),
        text: None,
        select: Value::Null,
        direction: None,
        amount: None,
        window: None,
        allow_foreground: false,
        mechanism: Some(Mechanism::Ax),
        name: name.map(String::from),
    }
}

// ---------- HAND-551: the listed actions ----------

// decision: HAND-551
#[test]
fn an_apps_own_actions_are_listed_by_name_bounded_and_as_data() {
    assert_eq!(ax::action_shown("AXShowMenu").as_deref(), Some("menu"));
    assert_eq!(
        ax::action_shown("AXShowAlternateUI").as_deref(),
        Some("show_alternate_ui")
    );
    assert_eq!(ax::action_shown("AXFancy").as_deref(), Some("fancy"));
    assert_eq!(
        ax::action_shown("Name:Save As\u{2026}\nTarget:0x0\nSelector:(null)").as_deref(),
        Some("Save As\u{2026}")
    );
    // control characters and runs of space are one space; a long name is cut; an empty one isn't listed
    assert_eq!(
        ax::action_shown("Name:Reply\t\u{7}  all\nTarget:0x1").as_deref(),
        Some("Reply all")
    );
    let long = format!("Name:{}\nTarget:0x0", "x".repeat(200));
    assert_eq!(
        ax::action_shown(&long).unwrap().chars().count(),
        ax::MAX_ACTION_NAME
    );
    assert_eq!(ax::action_shown("Name:\u{1}\nTarget:0x0"), None);
    // each once, at most MAX_ACTIONS
    let many: Vec<String> = (0..40)
        .map(|i| format!("Name:Act {i}\nTarget:0x0"))
        .chain(["AXPress".into(), "AXPress".into()])
        .collect();
    let shown = ax::shown_actions(&many);
    assert_eq!(shown.len(), ax::MAX_ACTIONS);
    let twice = ax::shown_actions(&["AXPress".to_string(), "AXPress".to_string()]);
    assert_eq!(twice, ["press"]);
}

// decision: HAND-551
#[tokio::test]
async fn an_elements_listed_action_is_performed_and_one_it_doesnt_list_is_refused() {
    let m = mac();
    let snap = m.snapshot().await;
    let els = &snap["elements"];
    let msg = els
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["label"] == "Message")
        .unwrap();
    assert_eq!(
        msg["actions"],
        json!(["press", "Save As\u{2026}", "Download", "show_alternate_ui"])
    );
    let (msg_id, send_id) = (Mac::id(els, "Message"), Mac::id(els, "Send"));
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "perform", "element": msg_id, "name": "Download"}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert!(
        m.calls()
            .contains(&"Name:Download\nTarget:0x0\nSelector:(null) msg".to_string()),
        "{:?}",
        m.calls()
    );
    // a name the element doesn't list, and an action the button doesn't advertise: refused, nothing performed
    let before = m.calls().len();
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "perform", "element": msg_id, "name": "Forward"}),
        )
        .await;
    assert!(r.is_error);
    assert_eq!(
        r.content["error"],
        format!("{msg_id} doesn't list the action 'Forward' now (it lists: press, Save As\u{2026}, Download, show_alternate_ui); nothing was done; take a new snapshot")
    );
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "menu", "element": send_id}),
        )
        .await;
    assert!(r.is_error);
    assert!(
        r.content["error"]
            .as_str()
            .unwrap()
            .contains("doesn't list the action 'menu'"),
        "{:?}",
        r.content
    );
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "perform", "element": send_id}),
        )
        .await;
    assert_eq!(
        r.content["error"],
        ferb_mac_hands::driver::PERFORM_NEEDS_NAME
    );
    assert_eq!(m.calls().len(), before, "{:?}", m.calls());
    // perform naming one of mac.act's own words is that action: press goes the press route
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "perform", "element": send_id, "name": "press"}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert_eq!(m.calls().last().unwrap(), "AXPress send");
}

// ---------- HAND-552: the context menu ----------

// decision: HAND-552, HAND-551
#[tokio::test]
async fn a_menu_opens_in_the_background_its_items_come_back_and_one_is_pressed() {
    let m = mac();
    let snap = m.snapshot().await;
    let els = &snap["elements"];
    let doc = els
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["label"] == "Document bubble")
        .unwrap();
    // the live case's element: its listed actions, "menu" among them
    assert_eq!(
        doc["actions"],
        json!(["press", "scroll_to_visible", "cancel", "menu"])
    );
    let doc_id = doc["id"].as_str().unwrap().to_string();
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "menu", "element": doc_id}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert_eq!(m.calls(), ["AXShowMenu doc"]);
    assert_eq!(r.content["facts"]["menu_open"], json!(true));
    assert_eq!(r.content["next"], ferb_mac_hands::tools::MENU_NEXT);
    let items = &r.content["facts"]["elements"];
    let labels: Vec<&str> = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["role"] == "menuitem")
        .map(|e| e["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Save As\u{2026}", "Delete", "Quit Chat"]);
    assert!(
        items.as_array().unwrap().iter().all(|e| e["in"] == "menu"),
        "{items}"
    );
    // the earlier snapshot's ids no longer hold
    let old = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "press", "element": doc_id}),
        )
        .await;
    assert!(old.is_error, "{:?}", old.content);
    // the next snapshot reads the open menu too
    let again = m.snapshot().await;
    assert!(
        again["open"].as_array().unwrap().contains(&json!("menu")),
        "{again}"
    );
    let delete = Mac::id(&again["elements"], "Delete");
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "press", "element": delete}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert_eq!(m.calls().last().unwrap(), "AXPress mi_delete");
    // nothing was posted, nothing activated, and the owner's notes app stayed in front
    assert!(m.posted.lock().unwrap().is_empty());
    assert!(m.apps.calls().is_empty());
    assert_eq!(m.apps.front(), NOTES);
}

// decision: HAND-552
#[tokio::test]
async fn cancel_on_the_open_menu_closes_it() {
    let m = mac();
    let snap = m.snapshot().await;
    let doc = Mac::id(&snap["elements"], "Document bubble");
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "menu", "element": doc}),
        )
        .await;
    let menu = r.content["facts"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["role"] == "menu")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "cancel", "element": menu}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert_eq!(m.calls().last().unwrap(), "AXCancel ctx");
    let after = m.snapshot().await;
    assert!(after.get("open").is_none(), "{after}");
}

// decision: HAND-552
#[tokio::test]
async fn a_menu_waits_for_the_owner_to_be_quiet_first() {
    let m = mac();
    let snap = m.snapshot().await;
    let doc = Mac::id(&snap["elements"], "Document bubble");
    // the owner keeps typing: the menu isn't opened under them, and the call yields
    m.owner_active.store(true, Ordering::SeqCst);
    let r = m
        .driver
        .act(
            "Chat",
            &element(K::Menu, &doc, None),
            "w1",
            Some("t1"),
            None,
        )
        .await;
    assert!(matches!(r, Err(HandError::Yielded { .. })), "{r:?}");
    assert!(m.calls().is_empty(), "{:?}", m.calls());
    assert_eq!(*m.waited.lock().unwrap(), [true, false]);
    // once quiet it opens
    m.owner_active.store(false, Ordering::SeqCst);
    let r = m
        .driver
        .act(
            "Chat",
            &element(K::Menu, &doc, None),
            "w1",
            Some("t1"),
            None,
        )
        .await
        .unwrap();
    assert!(r.ok, "{}", r.detail);
}

// decision: HAND-219, HAND-551
#[tokio::test]
async fn a_context_menu_items_quit_shortcut_is_guarded_as_any_menu_press() {
    let m = mac();
    let snap = m.snapshot().await;
    let doc = Mac::id(&snap["elements"], "Document bubble");
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "menu", "element": doc}),
        )
        .await;
    let quit = Mac::id(&r.content["facts"]["elements"], "Quit Chat");
    for action in ["press", "perform"] {
        let r = m
            .run(
                "mac.act",
                json!({"app": "Chat", "action": action, "element": quit, "name": "press"}),
            )
            .await;
        assert!(
            r.is_error || r.content["ok"] == json!(false),
            "{action}: {:?}",
            r.content
        );
    }
    assert!(
        !m.calls().iter().any(|c| c == "AXPress mi_quit"),
        "{:?}",
        m.calls()
    );
}

// decision: HAND-238, HAND-551
#[tokio::test]
async fn ferbs_own_app_is_refused_for_every_element_action() {
    let m = mac();
    for kind in [K::Menu, K::Perform] {
        let r = m
            .driver
            .act(
                "FERB Notch",
                &element(kind, "s1e1", Some("press")),
                "w1",
                Some("t1"),
                None,
            )
            .await;
        match r {
            Err(HandError::Refused(why)) => assert_eq!(why, OWN_APP),
            other => panic!("{other:?}"),
        }
    }
    assert!(m.calls().is_empty());
}

// decision: HAND-220, HAND-551
#[tokio::test]
async fn an_element_action_is_classified_and_identified_as_a_press_on_it() {
    let m = mac();
    let snap = m.snapshot().await;
    let msg = Mac::id(&snap["elements"], "Message");
    let press = json!({"app": "Chat", "action": "press", "element": msg});
    let perform = json!({"app": "Chat", "action": "perform", "element": msg, "name": "Download"});
    let menu = json!({"app": "Chat", "action": "menu", "element": msg});
    // the same kind of action for the confirmations, on the same app
    for args in [&perform, &menu] {
        let a = m.tools.describe("mac.act", args);
        let p = m.tools.describe("mac.act", &press);
        assert_eq!(
            (a.kind.as_str(), &a.target.site),
            (p.kind.as_str(), &p.target.site)
        );
    }
    // the ledger's identity: the element as it is (HAND-220's window, role path, role, label), plus the action's name
    let hashed = |v: &Value| v.as_object().unwrap().clone();
    let a = m.tools.ledger_args("mac.act", &perform, &hashed(&perform));
    let p = m.tools.ledger_args("mac.act", &press, &hashed(&press));
    assert_eq!(a["element"], p["element"]);
    assert_eq!(a["element"]["label"], "Message");
    assert_eq!(
        (a["action"].clone(), a["name"].clone()),
        (json!("perform"), json!("Download"))
    );
}

// ---------- HAND-549, HAND-553, HAND-554: an app that comes to the front ----------

async fn shot(m: &Mac) -> String {
    let r = m.run("mac.screenshot", json!({"app": "Chat"})).await;
    assert!(!r.is_error, "{:?}", r.content);
    r.content["shot"].as_str().unwrap().to_string()
}

// decision: HAND-549, HAND-553, HAND-554
#[tokio::test]
async fn an_app_that_raises_itself_on_a_click_is_put_back_told_once_and_remembered() {
    let m = mac();
    m.owner_active.store(true, Ordering::SeqCst);
    m.pointer_raises.store(true, Ordering::SeqCst);
    let s = shot(&m).await;
    let err = m
        .tools
        .run(
            "mac.pointer",
            &json!({"app": "Chat", "shot": s, "x": 50, "y": 20, "action": "rightclick"}),
            &ctx("t1"),
        )
        .await
        .unwrap_err();
    let HandError::PointerInterrupted(text) = err else {
        panic!("{err:?}")
    };
    assert_eq!(
        text,
        "pointer input interrupted; outcome unknown: Chat came to the front by itself when it got FERB's rightclick (it \
wasn't in front before), and the owner is using the Mac, so the rest of the pointer input stopped. Chat came to the \
front by itself when it got FERB's input (it wasn't in front before); FERB put Notes back in front. From now on FERB \
asks the owner before sending Chat clicks, pointer or key events; the actions its elements list in mac_snapshot \
(mac_act press, menu and the others) stay in the background and don't ask."
    );
    assert_eq!(*m.posted.lock().unwrap(), ["rightclick down"]);
    // the owner's app is back in front, FERB's activation of it attributed to FERB
    assert_eq!(m.apps.calls(), [format!("activate {NOTES_ID}")]);
    assert_eq!(m.apps.front(), NOTES);
    // the owner's line: one fact for the notice
    assert_eq!(
        *m.told.lock().unwrap(),
        [(
            "t1".to_string(),
            json!({"cause": "front_app", "app": "Chat", "put_back": true, "previous": "Notes", "input": "raw",
                   "action": "click"})
        )]
    );
    // remembered: the gateway asks before the next raw input to it, in this task and later ones
    assert!(m.driver.profiles.comes_forward(CHAT_ID));
    assert_eq!(
        m.driver
            .profiles
            .status(CHAT_ID, RAW_INPUT_BACKGROUND)
            .as_deref(),
        Some("failed")
    );
}

// decision: HAND-553
#[tokio::test]
async fn an_app_the_owner_is_using_keeps_the_owner_text_and_is_not_put_back() {
    let m = mac();
    *m.apps.front.lock().unwrap() = CHAT;
    m.owner_active.store(true, Ordering::SeqCst);
    let s = shot(&m).await;
    let err = m
        .tools
        .run(
            "mac.pointer",
            &json!({"app": "Chat", "shot": s, "x": 50, "y": 20, "action": "click"}),
            &ctx("t1"),
        )
        .await;
    let text = match err {
        Ok(r) => r.content["error"].as_str().unwrap_or("").to_string(),
        Err(e) => e.to_string(),
    };
    assert!(
        text.contains("the owner is (or may be) using this app; pointer input stopped"),
        "{text}"
    );
    assert!(m.apps.calls().is_empty());
    assert!(m.told.lock().unwrap().is_empty());
    assert_eq!(
        m.driver.profiles.status(CHAT_ID, RAW_INPUT_BACKGROUND),
        None
    );
}

// decision: HAND-549, HAND-554
#[tokio::test]
async fn an_app_that_stays_behind_changes_nothing_and_clears_the_record() {
    let m = mac();
    m.driver
        .profiles
        .record(CHAT_ID, RAW_INPUT_BACKGROUND, false);
    let s = shot(&m).await;
    let r = m
        .run(
            "mac.pointer",
            json!({"app": "Chat", "shot": s, "x": 50, "y": 20, "action": "click"}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert!(r.content.get("screen_changed_during_call").is_none());
    assert_eq!(*m.posted.lock().unwrap(), ["click down", "click up"]);
    assert!(m.apps.calls().is_empty());
    assert!(m.told.lock().unwrap().is_empty());
    assert_eq!(m.apps.front(), NOTES);
    // the last observation decides: it stayed behind, so raw input to it no longer asks
    assert!(!m.driver.profiles.comes_forward(CHAT_ID));
}

// decision: HAND-549, HAND-554
#[tokio::test]
async fn an_app_raised_by_an_ax_action_is_put_back_but_ax_actions_still_dont_ask() {
    let m = mac();
    m.menu_raises();
    let snap = m.snapshot().await;
    let doc = Mac::id(&snap["elements"], "Document bubble");
    let r = m
        .run(
            "mac.act",
            json!({"app": "Chat", "action": "menu", "element": doc}),
        )
        .await;
    assert!(!r.is_error, "{:?}", r.content);
    assert_eq!(m.apps.calls(), [format!("activate {NOTES_ID}")]);
    assert_eq!(m.apps.front(), NOTES);
    let fact = r.content["screen_changed_during_call"].clone();
    assert_eq!(
        fact["front_app"],
        json!({"before": "Notes", "after": "Chat"})
    );
    assert_eq!(fact["put_back"], json!(true));
    assert_eq!(fact["raw_input_asks"], json!(false));
    assert_eq!(
        m.driver.profiles.status(CHAT_ID, RAW_INPUT_BACKGROUND),
        None
    );
}

/// HAND-560: the browser calls' put-back is this one: the app that was in front goes back while the app that came
/// forward is still in front; once someone switched to another app, nothing is activated and the reason is given.
// decision: HAND-560
#[tokio::test]
async fn the_put_back_a_browser_call_uses_skips_when_someone_switched_since() {
    let m = mac();
    let notes = RunningApp::new(NOTES_ID, "Notes", NOTES, false);
    let chat = RunningApp::new(CHAT_ID, "Chat", CHAT, true);
    *m.apps.front.lock().unwrap() = CHAT;
    assert_eq!(m.tools.put_back(&chat, &notes, "t1").await, (true, None));
    assert_eq!(m.apps.calls(), [format!("activate {NOTES_ID}")]);
    assert_eq!(m.apps.front(), NOTES);
    // the owner went to another app meanwhile: left as it is
    *m.apps.front.lock().unwrap() = OTHER;
    assert_eq!(
        m.tools.put_back(&chat, &notes, "t1").await,
        (false, Some("another app is in front now".to_string()))
    );
    assert_eq!(m.apps.calls().len(), 1);
    assert_eq!(m.apps.front(), OTHER);
}

/// HAND-560: the owner's line names what FERB did in the app.
// decision: HAND-560
#[test]
fn the_input_is_named_by_what_it_did() {
    use ferb_mac_hands::tools::input_action;
    assert_eq!(input_action("mac.click", &json!({})), "click");
    assert_eq!(
        input_action("mac.pointer", &json!({"action": "rightclick"})),
        "click"
    );
    assert_eq!(
        input_action("mac.pointer", &json!({"action": "drag"})),
        "drag"
    );
    assert_eq!(
        input_action("mac.pointer", &json!({"action": "scroll"})),
        "scroll"
    );
    assert_eq!(
        input_action("mac.act", &json!({"action": "type", "text": "hi"})),
        "type"
    );
    assert_eq!(
        input_action("mac.act", &json!({"action": "key", "key": "cmd+s"})),
        "type"
    );
    assert_eq!(input_action("mac.act", &json!({"action": "menu"})), "use");
    assert_eq!(input_action("mac.menu", &json!({})), "use");
}

// decision: HAND-549, HAND-554
#[tokio::test]
async fn foreground_input_the_owner_allowed_is_never_put_back() {
    let m = mac();
    let r = m
        .run("mac.act", json!({"app": "Chat", "action": "type", "text": "hi", "mechanism": "foreground", "allow_foreground": true}))
        .await;
    // FERB brought Chat forward itself with the owner's Yes: it stays
    assert!(!r.is_error, "{:?}", r.content);
    assert_eq!(m.apps.front(), CHAT);
    assert_eq!(m.apps.calls(), [format!("activate {CHAT_ID}")]);
    assert!(m.told.lock().unwrap().is_empty());
}

// decision: HAND-554
#[test]
fn raw_input_is_pointer_and_key_events_not_element_actions() {
    use ferb_mac_hands::tools::raw_input;
    assert!(raw_input("mac.click", &json!({"app": "Chat"})));
    assert!(!raw_input(
        "mac.click",
        &json!({"app": "Chat", "mechanism": "ax"})
    ));
    assert!(raw_input("mac.pointer", &json!({"action": "rightclick"})));
    assert!(!raw_input("mac.pointer", &json!({"action": "hover"})));
    assert!(raw_input(
        "mac.act",
        &json!({"action": "type", "text": "x", "element": "s1e1"})
    ));
    assert!(raw_input(
        "mac.act",
        &json!({"action": "key", "text": "return"})
    ));
    for a in ["press", "menu", "perform", "set_value", "cancel", "scroll"] {
        assert!(!raw_input("mac.act", &json!({"action": a})), "{a}");
    }
    assert!(raw_input(
        "mac.act",
        &json!({"action": "scroll", "mechanism": "pid"})
    ));
}

// decision: HAND-553
#[test]
fn the_stop_names_the_app_that_came_forward() {
    assert_eq!(
        ferb_mac_hands::driver::came_forward_stop("WhatsApp", "rightclick"),
        "WhatsApp came to the front by itself when it got FERB's rightclick (it wasn't in front before), and the owner \
is using the Mac, so the rest of the pointer input stopped"
    );
}

// ---------- by hand, on a real app ----------

/// By hand only (never in CI, never while the owner works): with TextEdit open on an untitled document, opens the
/// text area's context menu through Accessibility in the background, prints its items and the front app before and
/// after, and closes it with cancel. `cargo test -p ferb-mac-hands --test element_actions -- --ignored --nocapture`.
#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore]
async fn by_hand_textedit_context_menu() {
    use ferb_mac_hands::native;
    let ax = native::ax::AxDriver::new(|_| false);
    let running = native::apps::running();
    let te = running
        .iter()
        .find(|a| a.bundle_id == "com.apple.TextEdit")
        .expect("open TextEdit first");
    let front = || {
        native::apps::running()
            .into_iter()
            .find(|a| a.active)
            .map(|a| a.name)
    };
    let tree = ax.read_tree(te.pid, false, None, 0, None, false);
    let area: &AxNode = tree
        .nodes
        .iter()
        .find(|n| n.ax_role == "AXTextArea" && n.actions.iter().any(|a| a == "menu"))
        .expect("a text area that lists menu");
    println!("actions: {:?}; front before: {:?}", area.actions, front());
    let shown = ax.perform(&area.r#ref, "AXShowMenu");
    println!("AXShowMenu: {shown:?}; front after: {:?}", front());
    let menu = ax.open_menu(&area.r#ref, te.pid).expect("an open menu");
    println!(
        "items: {:?}",
        ax.menu_items(&menu)
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
    );
    println!(
        "cancel: {:?}; front at the end: {:?}",
        ax.cancel(&menu),
        front()
    );
}
