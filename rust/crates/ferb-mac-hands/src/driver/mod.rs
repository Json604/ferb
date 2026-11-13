//! Native target references and explicit AX / process / foreground input mechanics (ferb/act/mac/driver.py). Agents
//! choose methods and retries; this driver never substitutes targets, replays uncertain input or restores focus.
//!
//! The Mac itself comes in through [`Ax`], [`Apps`], [`Shots`], `input::Gestures` and the policy facts through
//! [`Policy`]; `native` implements the first four, the policy crate the last.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ferb_core::types::{
    ActionFacts, ActionResult, ComputerAction, ComputerActionKind as K, Mechanism, ReceivedBy,
    TerminalGrant,
};
use serde_json::{json, Map, Value};
use sha2::Digest;

use crate::apps::RunningApp;
use crate::ax::{self, AxNode, AxTree, El, Rect, ScrollPosition, Signature};
use crate::error::{refused, stale, HandError, Result};
use crate::input::{off, Gestures, RealInput, Step};

use crate::launched::LaunchedApps;
use crate::mask::{self, Look, WebSecrets};
use crate::point::{preview, preview_left, preview_phase, Frame, FrameFn, PREVIEW};
use crate::profile::Profiles;

// ---------- words ----------

pub const OWN_APP: &str = "FERB never operates its own app; questions are answered by the owner";
pub const RUNS_OPENED: &str = "a file that can run when opened";
pub const CANT_CONFIRM: &str =
    "FERB can't confirm what is under this point, so it didn't click: one of its own \
windows could be there (rule 8, rule 9)";
pub const WINDOW_WALK_NODES: usize = 2000;
pub const WINDOW_WALK_S: f64 = 0.2;
pub const SETTLE_S: f64 = 0.4;
pub const TYPE_CHUNK: usize = 8;
pub const FOCUS_ELSEWHERE: &str =
    "something else in the app has the keyboard focus (a dialog or sheet may be open), so \
nothing was typed; take a new snapshot";
pub const CLICK_GRID: f64 = 24.0;
pub const CLICK_ELEMENT_MAX: f64 = 400.0;
pub const SECURE_KEYS: &str =
    "that's a password field, and this key would put text into it; FERB never types \
passwords for the owner (the owner fills it, or mac.secret does from the Keychain)";
pub const WINDOW_GONE: &str = "the window moved or closed since the screenshot; take a new one";
pub const SECURE_TYPED: &str = "that's a password field; FERB never types passwords for the owner";
pub const NEEDS_ELEMENT: &str =
    "this action needs an element id from a snapshot (only type with mechanism foreground \
and allow_foreground true goes to whatever has the app's keyboard focus without one)";
pub const SHOT_AGE_S: f64 = 30.0;
pub const FRAME_RESERVE_S: f64 = 1.0;
pub const WALK_FLOOR_S: f64 = 0.2;
/// sense/front_window.py FERB_APPS: FERB's own apps by bundle id (casefolded).
pub const FERB_APPS: [&str; 6] = [
    "com.ferb.ferb",
    "dev.ferb.notch",
    "com.ferb.app",
    "com.ferb.daemon",
    "com.ferb.python",
    "com.ferb.secrets",
];
pub const UNTRUSTED_APPS: [(&str, &str); 7] = [
    ("com.apple.mail", "email"),
    ("com.apple.MobileSMS", "message"),
    ("net.whatsapp.WhatsApp", "message"),
    ("com.tinyspeck.slackmacgap", "message"),
    ("com.hnc.Discord", "message"),
    ("ru.keepcoder.Telegram", "message"),
    ("com.microsoft.teams2", "message"),
];

fn no_focus(app: &str) -> String {
    format!(
        "FERB couldn't read what has the keyboard focus in {app}, so it can't tell whether it is a password field and \
typed nothing; take a snapshot and type into an element"
    )
}

pub type Facts = Map<String, Value>;

/// The window server's facts about one window (screenshot.py:31).
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub id: i64,
    pub pid: i32,
    pub bounds: Rect,
    pub title: String,
    pub layer: i64,
    pub alpha: f64,
}

impl Window {
    pub fn new(id: i64, pid: i32, bounds: Rect, title: &str) -> Window {
        Window {
            id,
            pid,
            bounds,
            title: title.into(),
            layer: 0,
            alpha: 1.0,
        }
    }
}

// ---------- the seams ----------

/// Accessibility as the driver uses it (accessibility.py through `ax::Ax`, or a test's fake). Methods with a default
/// stand for what a Python fake may lack (`getattr(self.ax, …, None)`): the default is what the driver does then.
#[allow(unused_variables)]
pub trait Ax: Send + Sync {
    fn read_tree(
        &self,
        pid: i32,
        browser: bool,
        window: Option<&El>,
        offset: usize,
        budget_s: Option<f64>,
        masks_only: bool,
    ) -> AxTree;
    fn expose_tree(&self, pid: i32) -> bool;
    fn app_window(&self, pid: i32) -> Option<El>;
    /// None: the source can't list windows (the driver asks `app_window`).
    fn ax_windows(&self, pid: i32) -> Option<Vec<El>> {
        None
    }
    fn windows(&self, pid: i32) -> Vec<Facts> {
        vec![]
    }
    fn window_named(&self, pid: i32, which: &str) -> Option<El> {
        None
    }
    fn frame(&self, el: &El) -> Option<Rect> {
        None
    }
    fn window_with_frame(
        &self,
        pid: i32,
        bounds: Rect,
        title: Option<&str>,
        current: Option<&dyn Fn() -> String>,
    ) -> Option<El> {
        None
    }
    fn element_window(&self, el: &El) -> Option<El> {
        None
    }
    fn window_title(&self, win: &El) -> String {
        String::new()
    }
    fn web_areas_of(&self, win: &El) -> Option<Vec<Rect>> {
        None
    }
    fn hit(&self, x: f64, y: f64) -> Option<(i32, Option<Rect>)> {
        None
    }
    fn hit_element(&self, pid: i32, x: f64, y: f64) -> Option<El> {
        None
    }
    fn parent_of(&self, el: &El) -> Option<El> {
        None
    }
    fn role_of(&self, el: &El) -> String {
        String::new()
    }
    fn subrole_of(&self, el: &El) -> String {
        String::new()
    }
    fn secure_element(&self, el: &El) -> bool {
        false
    }
    fn focused_element(&self, pid: i32) -> Option<El> {
        None
    }
    /// CFEqual; a fake without one compares its elements as values.
    fn same(&self, a: &El, b: &El) -> bool {
        el_eq(a, b)
    }
    /// None: the source can't read an element's facts at all.
    fn surface_facts(&self, el: &El) -> Option<Vec<Facts>> {
        None
    }
    /// None: the source can't walk a window's facts.
    fn window_facts(&self, el: &El) -> Option<Vec<Option<Facts>>> {
        None
    }
    fn active_editor(&self, pid: i32) -> Option<El> {
        None
    }
    fn selected_text(&self, el: &El) -> Option<String> {
        None
    }
    fn document_of(&self, el: &El) -> Option<String> {
        None
    }
    fn place_of(&self, el: &El) -> (String, String) {
        (String::new(), String::new())
    }
    fn file_of(&self, el: &El) -> Option<String> {
        None
    }
    fn selected_items(&self, pid: i32) -> Vec<El> {
        vec![]
    }
    /// The outer None: the source can't read page addresses at all.
    fn page_url(&self, el: &El) -> Option<Option<String>> {
        None
    }
    fn menu_item_shortcut(&self, pid: i32, path: &[String]) -> Option<(String, i64)> {
        None
    }
    fn item_shortcut(&self, el: &El) -> Option<(String, i64)> {
        None
    }
    fn press(&self, el: &El) -> Result<bool>;
    fn perform(&self, el: &El, action: &str) -> Result<bool> {
        Ok(false)
    }
    fn cancel(&self, el: &El) -> Result<bool> {
        Ok(false)
    }
    fn focus(&self, el: &El) -> bool;
    fn has_focus(&self, pid: i32, el: &El) -> Option<bool>;
    fn set_value(&self, el: &El, text: &str) -> Result<bool>;
    fn value(&self, el: &El) -> Option<String>;
    fn selected_range(&self, el: &El) -> Option<(i64, i64)> {
        None
    }
    fn set_selected_range(&self, el: &El, start: i64, length: i64) -> bool {
        false
    }
    fn set_selected_text(&self, el: &El, text: &str) -> bool {
        false
    }
    fn menus(&self, pid: i32) -> Vec<Value> {
        vec![]
    }
    fn press_menu(&self, pid: i32, path: &[String]) -> Result<bool> {
        Ok(false)
    }
    fn menu_item_for_shortcut(
        &self,
        pid: i32,
        key: &str,
        mods: &[String],
    ) -> Option<(El, String, bool)>;
    fn center(&self, n: &AxNode) -> (f64, f64) {
        ax::center(n)
    }
    fn raise_window(&self, pid: i32, bounds: Rect) -> bool {
        false
    }
    fn scroll_area_of(&self, el: &El) -> Option<El> {
        None
    }
    fn scroll_area_in(&self, win: &El) -> Option<El> {
        None
    }
    fn scroll_content(&self, area: &El) -> Option<El> {
        None
    }
    fn scroll_bar(&self, area: &El, vertical: bool) -> Option<El> {
        None
    }
    fn scroll_position(&self, area: Option<&El>, content: Option<&El>) -> ScrollPosition {
        ScrollPosition::default()
    }
    fn popup_menu(&self, el: &El, pid: i32) -> Option<El> {
        None
    }
    fn menu_items(&self, menu: &El) -> Vec<(El, String)> {
        vec![]
    }
}

/// Elements compared as values when the source has no CFEqual (the tests' string elements), else by identity.
pub fn el_eq(a: &El, b: &El) -> bool {
    match (a.get::<String>(), b.get::<String>()) {
        (Some(x), Some(y)) => x == y,
        _ => a.is(b),
    }
}

/// Running apps and Launch Services (apps.py; `native::apps`).
#[allow(unused_variables)]
pub trait Apps: Send + Sync {
    fn find(&self, q: &str) -> Option<RunningApp>;
    /// None: the source can't list near matches (Python's fakes without `candidates`).
    fn candidates(&self, q: &str) -> Option<Vec<RunningApp>> {
        None
    }
    fn running(&self) -> Vec<RunningApp> {
        vec![]
    }
    fn pids_of(&self, bundle_ids: &[&str]) -> Vec<i32> {
        vec![]
    }
    fn frontmost(&self) -> Option<RunningApp>;
    fn activate(&self, bundle_id: &str, pid: Option<i32>) -> bool;
    fn launch(&self, bundle_id: &str) -> bool {
        false
    }
    fn open_in(&self, bundle_id: &str, target: &str) -> Option<String> {
        Some(format!("no installed app {bundle_id}"))
    }
    fn reopen(&self, pid: i32) -> Option<String> {
        Some("not supported here".into())
    }
    /// None: the source can't tell (the check is skipped, as in Python without `is_application`).
    fn is_application(&self, path: &str) -> Option<bool> {
        None
    }
    fn opens_web_links(&self, bundle_id: &str) -> bool {
        false
    }
    fn known_opens_web_links(&self, bundle_id: &str) -> bool {
        false
    }
    fn declares_shell(&self, bundle_id: &str) -> bool {
        false
    }
    fn runs_when_opened(&self, path: &str) -> bool {
        false
    }
    fn edits(&self, bundle_id: &str, path: &str) -> bool {
        false
    }
    fn default_app_for(&self, path: &str) -> Option<String> {
        None
    }
}

/// The window server and ScreenCaptureKit (screenshot.py; `native::windows`, `native::capture`).
#[allow(unused_variables)]
pub trait Shots: Send + Sync {
    fn window(&self, id: i64) -> Option<Window>;
    fn front_window(&self, pid: i32, like: Option<Rect>, title: Option<&str>) -> Option<Window>;
    fn on_screen(&self, id: i64) -> bool;
    fn window_at(&self, x: f64, y: f64) -> Option<Window>;
    /// None: the window list couldn't be read (a check then fails closed).
    fn windows_at(&self, x: f64, y: f64) -> Option<Vec<Window>> {
        None
    }
    /// JPEG bytes of exactly this window and the image size; `max_edge` None: native pixels. `busy_ok`: a live
    /// preview's capture never waits for another (CaptureBusy).
    fn capture(
        &self,
        w: &Window,
        max_edge: Option<u32>,
        preview: bool,
    ) -> Result<(Vec<u8>, i64, i64)>;
    /// screenshot.py:47 (L33).
    fn window_on_screen(
        &self,
        pid: i32,
        rect: Rect,
        like: Option<Rect>,
        title: Option<&str>,
        window: Option<i64>,
    ) -> bool {
        false
    }
    /// The display holding a rect's centre (point.py:49).
    fn display_of(&self, rect: Rect) -> Option<u32> {
        None
    }
}

/// Policy facts the driver reads (ferb/policy/terminal.py, confirm.py `browser_page`, provenance.py
/// `typed_destination`): the policy crate's; tests replay what Python's answered.
pub trait Policy: Send + Sync {
    fn surface(
        &self,
        bundle_id: &str,
        chain: &[Facts],
        owner_apps: &[String],
        shell_app: bool,
        unread: bool,
    ) -> Option<String>;
    fn can_host(&self, bundle_id: &str, owner_apps: &[String], shell_app: bool) -> bool;
    fn run_menu(&self, path: &[String]) -> bool;
    fn terminal_menu(&self, title: &str) -> bool;
    fn run_control(&self, chain: &[Facts], editor: bool) -> Option<String>;
    fn editor_app(&self, bundle_id: &str) -> bool;
    fn structural(&self, facts: &Facts) -> bool;
    fn newline(&self, text: &str) -> bool;
    fn code_runner(&self, bundle_id: &str) -> Option<String>;
    fn opens_files(&self, bundle_id: &str) -> bool;
    fn unasked(&self, why: &str, app: &str) -> String;
    fn browser_page(&self, address: Option<&str>) -> bool;
    fn typed_destination(&self, text: &str) -> Option<String>;
}

/// The daemon's clock for the driver's waits.
pub trait Time: Send + Sync {
    fn monotonic(&self) -> f64;
    fn sleep(&self, seconds: f64) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub struct RealTime(pub std::time::Instant);

impl Default for RealTime {
    fn default() -> Self {
        RealTime(std::time::Instant::now())
    }
}

impl Time for RealTime {
    fn monotonic(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
    fn sleep(&self, seconds: f64) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            seconds.max(0.0),
        )))
    }
}

/// The browser extension's page inventory for a window (L13): (browser key, bounds, title) -> its answer.
pub type WebSecretsFn = Arc<
    dyn Fn(String, Rect, String) -> Pin<Box<dyn Future<Output = Option<WebSecrets>> + Send>>
        + Send
        + Sync,
>;
/// The trail's attribution (P4): while it is alive, events in that app are FERB's.
pub type Acting = Arc<dyn Fn(&str, Option<&str>) -> Box<dyn Any + Send> + Send + Sync>;
/// The secret prompt (mac.secret): (task, scope, handle, remember, shown, field) -> the value, or None declined.
pub type Clipboard = Arc<dyn Fn() -> (i64, Option<String>, bool) + Send + Sync>;

// ---------- state ----------

#[derive(Debug, Clone)]
pub struct Shot {
    pub id: String,
    pub window: i64,
    pub bounds: Rect,
    pub size: (i64, i64),
    pub pid: i32,
    pub at: f64,
    pub signature: Option<Signature>,
    pub masking: String,
    pub tab: Option<(String, String)>,
    pub nodes: Vec<AxNode>,
    pub title: String,
    pub masks: Vec<Rect>,
}

#[derive(Debug, Clone, Default)]
struct AppState {
    pid: i32,
    gen: u64,
    nodes: Vec<(String, AxNode)>,
    shots: u64,
    shot: Option<Shot>,
    tree: Option<AxTree>,
    tree_at: f64,
    window_id: Option<Value>,
}

impl AppState {
    fn node(&self, id: &str) -> Option<&AxNode> {
        self.nodes.iter().find(|(k, _)| k == id).map(|(_, n)| n)
    }
}

#[derive(Debug, Clone)]
struct Armed {
    el: Option<El>,
    confirmation_id: String,
    task: Option<String>,
}

fn same_rect(a: &Rect, b: &Rect) -> bool {
    (a.0 - b.0).abs() <= 1.0
        && (a.1 - b.1).abs() <= 1.0
        && (a.2 - b.2).abs() <= 1.0
        && (a.3 - b.3).abs() <= 1.0
}

/// driver.py:306: the smallest element of a walk whose frame holds this point.
pub fn node_at(nodes: &[AxNode], x: f64, y: f64) -> Option<&AxNode> {
    nodes
        .iter()
        .filter(|n| {
            let r = n.rect;
            r.2 > 0.0 && r.3 > 0.0 && r.0 <= x && x < r.0 + r.2 && r.1 <= y && y < r.1 + r.3
        })
        .fold(None, |best: Option<&AxNode>, n| match best {
            Some(b) if b.rect.2 * b.rect.3 <= n.rect.2 * n.rect.3 => Some(b),
            _ => Some(n),
        })
}

/// driver.py:314: screenshot pixels to global screen points.
pub fn shot_to_screen(shot: &Shot, x: f64, y: f64) -> (f64, f64) {
    let (bx, by, bw, bh) = shot.bounds;
    (
        bx + x * bw / shot.size.0 as f64,
        by + y * bh / shot.size.1 as f64,
    )
}

/// The routes each element action takes (driver.py:323).
pub fn element_routes(kind: K) -> &'static [&'static str] {
    match kind {
        K::Press | K::Click => &["ax", "foreground"],
        K::Type => &["foreground", "pid"],
        K::SetValue | K::Edit => &["ax", "ax_value"],
        K::Cancel | K::Focus | K::Select => &["ax"],
        K::Scroll => &["ax", "foreground", "pid"],
        K::Key => &["ax", "foreground", "pid"],
    }
}

pub fn mech_str(m: Mechanism) -> &'static str {
    match m {
        Mechanism::Ax => "ax",
        Mechanism::Pid => "pid",
        Mechanism::AxValue => "ax_value",
        Mechanism::Foreground => "foreground",
    }
}

pub fn kind_str(k: K) -> &'static str {
    match k {
        K::Press => "press",
        K::SetValue => "set_value",
        K::Focus => "focus",
        K::Click => "click",
        K::Type => "type",
        K::Key => "key",
        K::Edit => "edit",
        K::Scroll => "scroll",
        K::Select => "select",
        K::Cancel => "cancel",
    }
}

fn walk_budget(left: Option<f64>, walks: f64) -> Option<f64> {
    left.map(|l| WALK_FLOOR_S.max((l - FRAME_RESERVE_S) / walks))
}

fn pastes(parts: &[String]) -> bool {
    parts.last().is_some_and(|k| k == "v") && parts[..parts.len() - 1].iter().any(|m| m == "cmd")
}

fn pastes_item(shortcut: Option<(String, i64)>) -> bool {
    shortcut.is_some_and(|(c, m)| ferb_sanitize::parity::lower(&c) == "v" && m & 8 == 0)
}

/// HAND-519: `type` with no element on the foreground route.
pub fn focused_typing(action: &ComputerAction) -> bool {
    action.kind == K::Type
        && action.element.as_deref().unwrap_or("").is_empty()
        && action.text.is_some()
        && action.mechanism == Some(Mechanism::Foreground)
}

fn default_mechanism(action: &ComputerAction) -> &'static str {
    match action.mechanism {
        Some(m) => mech_str(m),
        None => {
            if matches!(action.kind, K::Type | K::Key) {
                "pid"
            } else {
                "ax"
            }
        }
    }
}

/// driver.py:377 (L33): whether `act` would deliver this action to `n`.
pub fn acts_on(action: &ComputerAction, n: &AxNode) -> bool {
    if action.kind == K::Key
        || matches!(action.kind, K::Type | K::SetValue | K::Select) && action.text.is_none()
        || action.kind == K::Select
            && ferb_py::value::strip(action.text.as_deref().unwrap_or("")).is_empty()
    {
        return false;
    }
    let mechanism = match action.mechanism {
        Some(m) => mech_str(m),
        None => {
            if action.kind == K::Type {
                "pid"
            } else {
                "ax"
            }
        }
    };
    if mechanism == "foreground" && !action.allow_foreground {
        return false;
    }
    if n.secret && matches!(action.kind, K::SetValue | K::Type | K::Edit) {
        return false;
    }
    element_routes(action.kind).contains(&mechanism) && action.kind != K::Key
}

fn ok_result(ok: bool, detail: impl Into<String>, facts: Value) -> ActionResult {
    ActionResult {
        ok,
        detail: detail.into(),
        facts: facts.as_object().cloned().unwrap_or_default(),
        observed: None,
    }
}

fn with(mut r: ActionResult, facts: Option<ActionFacts>) -> ActionResult {
    if r.observed.is_none() {
        r.observed = facts;
    }
    r
}

/// Python's `repr()` of a str.
pub fn py_repr(s: &str) -> String {
    crate::locks::ferb_py_repr(s)
}

fn fmt0(v: f64) -> String {
    format!("{:.0}", v.round_ties_even())
}

// ---------- the driver ----------

pub struct MacDriver {
    pub device_id: String,
    pub input: RealInput,
    pub gestures: Arc<dyn Gestures>,
    pub excluded: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub ax: Arc<dyn Ax>,
    pub apps: Arc<dyn Apps>,
    pub shots: Arc<dyn Shots>,
    pub policy: Arc<dyn Policy>,
    pub time: Arc<dyn Time>,
    pub web_secrets: Option<WebSecretsFn>,
    pub profiles: Arc<Profiles>,
    pub on_activate: Arc<dyn Fn(&str, Option<&str>) + Send + Sync>,
    pub acting: Option<Acting>,
    pub snapshot_images: Arc<dyn Fn() -> bool + Send + Sync>,
    pub terminal_apps: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub clipboard: Clipboard,
    pub launched: Option<Arc<LaunchedApps>>,
    /// The SkyLight window-addressed route is available (window_input.py `load_route`).
    pub route: bool,
    /// This process (the daemon).
    pub own_pid: i32,
    /// The Launch Services browser list (browsers.py `_handlers`).
    pub handlers: Arc<crate::apps::WebHandlers>,
    state: Mutex<HashMap<String, AppState>>,
    exposed: Mutex<HashSet<i32>>,
    armed: Mutex<HashMap<i32, Armed>>,
    click_through: Mutex<HashMap<i32, HashSet<i64>>>,
}

pub struct Parts {
    pub device_id: String,
    pub input: RealInput,
    pub gestures: Arc<dyn Gestures>,
    pub ax: Arc<dyn Ax>,
    pub apps: Arc<dyn Apps>,
    pub shots: Arc<dyn Shots>,
    pub policy: Arc<dyn Policy>,
    pub time: Arc<dyn Time>,
}

type A = RunningApp;

impl MacDriver {
    pub fn new(p: Parts) -> MacDriver {
        MacDriver {
            device_id: p.device_id,
            input: p.input,
            gestures: p.gestures,
            excluded: Arc::new(Vec::new),
            ax: p.ax,
            apps: p.apps,
            shots: p.shots,
            policy: p.policy,
            time: p.time,
            web_secrets: None,
            profiles: Arc::new(Profiles::memory()),
            on_activate: Arc::new(|_, _| {}),
            acting: None,
            snapshot_images: Arc::new(|| true),
            terminal_apps: Arc::new(Vec::new),
            clipboard: Arc::new(|| (0, None, false)),
            launched: None,
            route: false,
            own_pid: std::process::id() as i32,
            handlers: Arc::new(crate::apps::WebHandlers::default()),
            state: Mutex::new(HashMap::new()),
            exposed: Mutex::new(HashSet::new()),
            armed: Mutex::new(HashMap::new()),
            click_through: Mutex::new(HashMap::new()),
        }
    }

    async fn blk<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&MacDriver) -> T + Send + 'static,
    ) -> T {
        let d = self.clone();
        off(move || f(&d)).await
    }

    async fn sleep(&self, s: f64) {
        self.time.sleep(s).await
    }

    fn now(&self) -> f64 {
        self.time.monotonic()
    }

    // ---------- apps ----------

    pub fn own_app(&self, bundle_id: &str, pid: i32) -> bool {
        FERB_APPS.contains(&ferb_sanitize::parity::casefold(bundle_id).as_str())
            || pid == self.own_pid
    }

    /// driver.py:422: the one app a name, bundle id or pid:N names; refused when ambiguous, absent, excluded or FERB's.
    pub fn app(&self, name_or_bundle: &str) -> Result<A> {
        let Some(a) = self.apps.find(name_or_bundle) else {
            let candidates = self.apps.candidates(name_or_bundle).unwrap_or_default();
            if !candidates.is_empty() {
                let listed: Vec<String> = candidates
                    .iter()
                    .map(|c| format!("{} {} pid:{}", c.name, c.bundle_id, c.pid))
                    .collect();
                return refused(format!(
                    "ambiguous app identity; choose pid from mac.apps: {}",
                    listed.join("; ")
                ));
            }
            return refused(format!("{name_or_bundle} isn't running (open it first)"));
        };
        let ex: Vec<String> = (self.excluded)()
            .iter()
            .map(|e| ferb_sanitize::parity::lower(e))
            .collect();
        if ex.contains(&ferb_sanitize::parity::lower(&a.bundle_id))
            || ex.contains(&ferb_sanitize::parity::lower(&a.name))
        {
            return refused(format!("{} is on the owner's exclusion list", a.name));
        }
        if self.own_app(&a.bundle_id, a.pid) {
            return refused(OWN_APP);
        }
        Ok(a)
    }

    async fn resolve(self: &Arc<Self>, name: &str) -> Result<A> {
        let n = name.to_string();
        self.blk(move |d| d.app(&n)).await
    }

    /// A web browser by the one list, never blocking.
    pub fn is_browser(&self, bundle_id: &str) -> bool {
        crate::apps::known(bundle_id) || self.apps.known_opens_web_links(bundle_id)
    }

    /// Blocking: `is_browser` with Launch Services asked afresh when stale.
    pub fn browser_now(&self, bundle_id: &str) -> bool {
        crate::apps::known(bundle_id) || self.apps.opens_web_links(bundle_id)
    }

    fn own_pids(&self) -> HashSet<i32> {
        let mut s: HashSet<i32> = self.apps.pids_of(&FERB_APPS).into_iter().collect();
        s.insert(self.own_pid);
        s
    }

    fn own_pid_is(&self, pid: i32) -> bool {
        pid == self.own_pid || self.apps.pids_of(&FERB_APPS).contains(&pid)
    }

    /// driver.py:474: the click-through windows a FERB app reports (fix round 4, H1 and N1).
    pub fn report_click_through(&self, pid: i32, windows: &[i64]) -> bool {
        let running: HashSet<i32> = self.apps.pids_of(&FERB_APPS).into_iter().collect();
        if !running.contains(&pid) {
            return false;
        }
        let mut ct = self.click_through.lock().expect("lock");
        ct.retain(|p, _| running.contains(p));
        ct.insert(pid, windows.iter().copied().collect());
        true
    }

    pub fn click_through(&self) -> HashMap<i32, HashSet<i64>> {
        self.click_through.lock().expect("lock").clone()
    }

    fn skipped(&self, own: &HashSet<i32>) -> HashMap<i32, HashSet<i64>> {
        let mut ct = self.click_through.lock().expect("lock");
        ct.retain(|p, _| own.contains(p));
        ct.clone()
    }

    fn decided(check: &str, why: Option<&str>) -> Option<String> {
        ferb_core::call_facts::note("point_checks", check);
        why.map(String::from)
    }

    /// driver.py:507: why a real click posted at this point is refused (a FERB window may take it), else None.
    pub fn own_point(
        &self,
        x: f64,
        y: f64,
        pid: Option<i32>,
        window: Option<i64>,
    ) -> Option<String> {
        let own = self.own_pids();
        let skipped = self.skipped(&own);
        let found = self.ax.hit(x, y);
        if let Some((p, _)) = &found {
            if !own.contains(p) {
                return Self::decided("hit_test", None);
            }
        }
        let windows = self.shots.windows_at(x, y);
        if found.is_some() {
            let mine: Vec<&Window> = windows
                .iter()
                .flatten()
                .filter(|w| own.contains(&w.pid))
                .collect();
            if mine.is_empty()
                || mine
                    .iter()
                    .any(|w| !skipped.get(&w.pid).is_some_and(|s| s.contains(&w.id)))
            {
                return Self::decided("hit_test", Some(OWN_APP));
            }
        }
        let Some(windows) = windows else {
            return Self::decided("refused: cant_tell", Some(CANT_CONFIRM));
        };
        let why = Self::list_decides(&windows, pid, window, &own, &skipped);
        Self::decided("window_list", why.as_deref())
    }

    fn list_decides(
        windows: &[Window],
        pid: Option<i32>,
        window: Option<i64>,
        own: &HashSet<i32>,
        skipped: &HashMap<i32, HashSet<i64>>,
    ) -> Option<String> {
        for w in windows {
            if skipped.get(&w.pid).is_some_and(|s| s.contains(&w.id)) {
                continue;
            }
            if own.contains(&w.pid) {
                return Some(CANT_CONFIRM.into());
            }
            if w.alpha <= 0.0 {
                continue;
            }
            if let Some(p) = pid {
                if w.pid == p
                    && (match window {
                        Some(id) => w.id == id,
                        None => w.layer == 0,
                    })
                {
                    return None;
                }
            }
        }
        None
    }

    async fn expose(self: &Arc<Self>, a: &A) {
        let first = self.exposed.lock().expect("lock").insert(a.pid);
        if first {
            let pid = a.pid;
            self.blk(move |d| d.ax.expose_tree(pid)).await;
        }
    }

    pub async fn menus(self: &Arc<Self>, name: &str) -> Result<Vec<Value>> {
        let a = self.resolve(name).await?;
        Ok(self.blk(move |d| d.ax.menus(a.pid)).await)
    }

    /// driver.py:564.
    pub async fn press_menu(
        self: &Arc<Self>,
        name: &str,
        path: &[String],
        terminal: Option<&TerminalGrant>,
        task: Option<&str>,
    ) -> Result<bool> {
        let a = self.resolve(name).await?;
        self.armed.lock().expect("lock").remove(&a.pid);
        let combo = {
            let (a2, p) = (a.clone(), path.to_vec());
            self.blk(move |d| d.menu_combo(&a2, &p)).await
        };
        if let Some(combo) = combo {
            if let Some(handled) = self.guarded_combo(&a, &combo, task).await? {
                return Ok(handled.ok);
            }
        }
        let grant = terminal.cloned();
        let joined = path.join(" > ");
        if self.policy.run_menu(path) {
            let (a2, g, j) = (a.clone(), grant.clone(), joined.clone());
            if let Some(r) = self
                .blk(move |d| d.run_refusal(&a2, g.as_ref(), Some(&j), None))
                .await
            {
                return refused(r);
            }
        } else {
            let (a2, g) = (a.clone(), grant.clone());
            let files = self
                .blk(move |d| d.files_refusal(&a2, &d.selected_files(&a2), g.as_ref()))
                .await;
            if let Some(r) = files {
                return refused(r);
            }
            let (a2, p) = (a.clone(), path.to_vec());
            let pastes = self.blk(move |d| d.menu_pastes(&a2, &p)).await;
            let a2 = a.clone();
            let terminal_app = !pastes && self.blk(move |d| d.terminal_app(&a2)).await;
            if pastes {
                let a2 = a.clone();
                let focus = self.blk(move |d| d.focus_of(&a2)).await;
                let f2 = focus.clone();
                if self.blk(move |d| d.secure(f2.as_ref())).await {
                    return refused(SECURE_KEYS);
                }
                let (a2, g, t) = (a.clone(), grant.clone(), task.map(String::from));
                let why = self
                    .blk(move |d| {
                        d.terminal_refusal(
                            &a2,
                            focus.as_ref(),
                            g.as_ref(),
                            t.as_deref(),
                            None,
                            Some("cmd+v"),
                            None,
                        )
                    })
                    .await;
                if let Some(r) = why {
                    return refused(r);
                }
            } else if terminal_app {
                let a2 = a.clone();
                let focus = self.blk(move |d| d.focus_of(&a2)).await;
                let (a2, g, t, j) = (
                    a.clone(),
                    grant.clone(),
                    task.map(String::from),
                    joined.clone(),
                );
                let why = self
                    .blk(move |d| {
                        d.terminal_refusal(
                            &a2,
                            focus.as_ref(),
                            g.as_ref(),
                            t.as_deref(),
                            None,
                            None,
                            Some(&j),
                        )
                    })
                    .await;
                if let Some(r) = why {
                    return refused(r);
                }
            }
        }
        let (pid, p) = (a.pid, path.to_vec());
        let ok = self.blk(move |d| d.ax.press_menu(pid, &p)).await?;
        self.profiles.record(&a.bundle_id, "menu_press", ok);
        Ok(ok)
    }

    /// driver.py:591: the app shows at least one window Accessibility can reach.
    pub async fn ensure_window(self: &Arc<Self>, bundle_id: &str, wait_s: f64) -> bool {
        let deadline = self.now() + wait_s;
        loop {
            let b = bundle_id.to_string();
            if let Some(a) = self.blk(move |d| d.apps.find(&b)).await {
                self.expose(&a).await;
                let pid = a.pid;
                let found = self
                    .blk(move |d| match d.ax.ax_windows(pid) {
                        Some(list) => !list.is_empty(),
                        None => d.ax.app_window(pid).is_some(),
                    })
                    .await;
                if found {
                    return true;
                }
            }
            if self.now() >= deadline {
                return false;
            }
            self.sleep(0.25).await;
        }
    }

    // ---------- the one walk ----------

    async fn walk(
        self: &Arc<Self>,
        a: &A,
        window: Option<El>,
        offset: usize,
        keep: bool,
        budget_s: Option<f64>,
        masks_only: bool,
    ) -> AxTree {
        let browser = self.is_browser(&a.bundle_id);
        let keep = keep && !masks_only;
        let pid = a.pid;
        let tree = self
            .blk(move |d| {
                d.ax.read_tree(pid, browser, window.as_ref(), offset, budget_s, masks_only)
            })
            .await;
        if !keep {
            return tree;
        }
        let now = self.now();
        let mut st = self.state.lock().expect("lock");
        let entry = st.entry(a.bundle_id.clone()).or_default();
        if entry.pid != 0 && entry.pid != a.pid {
            *entry = AppState {
                gen: entry.gen,
                ..Default::default()
            };
        }
        entry.pid = a.pid;
        entry.tree = Some(tree.clone());
        entry.tree_at = now;
        tree
    }

    async fn signature(self: &Arc<Self>, a: &A, el: Option<&El>) -> Signature {
        let win = match el {
            Some(e) => {
                let e = e.clone();
                self.blk(move |d| d.ax.element_window(&e)).await
            }
            None => None,
        };
        self.walk(a, win, 0, true, None, false).await.signature()
    }

    fn window_signature(&self, a: &A, shot: &Shot) -> Option<Signature> {
        let win = self.ax.window_with_frame(
            a.pid,
            shot.bounds,
            if shot.title.is_empty() {
                None
            } else {
                Some(&shot.title)
            },
            None,
        )?;
        Some(
            self.ax
                .read_tree(
                    a.pid,
                    self.is_browser(&a.bundle_id),
                    Some(&win),
                    0,
                    None,
                    false,
                )
                .signature(),
        )
    }

    /// driver.py:659: mac.snapshot.
    pub async fn snapshot(
        self: &Arc<Self>,
        name: &str,
        window: Option<&str>,
        subtree: Option<&str>,
        offset: usize,
    ) -> Result<(A, Map<String, Value>)> {
        let a = self.resolve(name).await?;
        let b = a.bundle_id.clone();
        self.blk(move |d| d.browser_now(&b)).await;
        self.expose(&a).await;
        let mut win: Option<El> = None;
        let mut subtree_node: Option<AxNode> = None;
        if let Some(w) = window.filter(|w| !w.is_empty()) {
            let (pid, w2) = (a.pid, w.to_string());
            win = self.blk(move |d| d.ax.window_named(pid, &w2)).await;
            if win.is_none() {
                let listed = self.blk(move |d| d.ax.windows(pid)).await;
                let names: Vec<String> = listed
                    .iter()
                    .map(|w| {
                        let t = w
                            .get("title")
                            .map(ferb_py::value::py_str)
                            .unwrap_or_default();
                        let t = if w.get("title").is_some_and(ferb_py::value::truthy) {
                            t
                        } else {
                            "(untitled)".into()
                        };
                        format!(
                            "{t} [{}]",
                            w.get("id").map(ferb_py::value::py_str).unwrap_or_default()
                        )
                    })
                    .collect();
                let names = if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                };
                return refused(format!(
                    "{} has no window {} that Accessibility can reach right now (its windows: {names}); no window was \
substituted or brought forward",
                    a.name,
                    py_repr(w)
                ));
            }
        }
        if let Some(sub) = subtree.filter(|s| !s.is_empty()) {
            {
                let st = self.state.lock().expect("lock");
                if let Some(s) = st.get(&a.bundle_id) {
                    if s.pid != a.pid {
                        return stale("element process changed; take a new snapshot");
                    }
                }
            }
            if window.is_some_and(|w| !w.is_empty()) {
                return refused("choose window or subtree, not both");
            }
            let n = self.node(&a.bundle_id, sub)?;
            win = Some(n.r#ref.clone());
            subtree_node = Some(n);
        }
        let tree = self.walk(&a, win, offset, true, None, false).await;
        let profile = self.profiles.as_dict(&a.bundle_id);
        let mut st = self.state.lock().expect("lock");
        let s = st.entry(a.bundle_id.clone()).or_default();
        s.gen += 1;
        let mut nodes = tree.nodes.clone();
        if let Some(sn) = &subtree_node {
            for n in nodes.iter_mut() {
                let mut path = sn.path.clone();
                path.extend(n.path.iter().cloned());
                if n.path.is_empty() {
                    n.index = sn.index;
                }
                n.path = path;
            }
        } else {
            s.window_id = tree
                .windows
                .iter()
                .find(|w| w.get("current").is_some_and(ferb_py::value::truthy))
                .map(|w| w.get("id").cloned().unwrap_or(Value::Null));
        }
        let gen = s.gen;
        s.nodes = nodes
            .into_iter()
            .enumerate()
            .map(|(i, n)| (format!("s{gen}e{}", i + 1), n))
            .collect();
        let mut els = vec![];
        for (eid, n) in &s.nodes {
            let mut d = Map::new();
            d.insert("id".into(), json!(eid));
            d.insert("role".into(), json!(n.role));
            d.insert("label".into(), json!(n.label));
            if !n.actions.is_empty() {
                d.insert("actions".into(), json!(n.actions));
            }
            if n.secret {
                d.insert("sensitive".into(), json!(true));
                d.insert("filled".into(), json!(n.value.as_deref() == Some("filled")));
            } else if let Some(v) = n.value.as_ref().filter(|v| !v.is_empty()) {
                d.insert("value".into(), json!(v));
            }
            if !n.enabled {
                d.insert("disabled".into(), json!(true));
            }
            if n.selected == Some(true) {
                d.insert("selected".into(), json!(true));
            }
            if let Some(e) = n.expanded {
                d.insert("expanded".into(), json!(e));
            }
            if let Some(c) = n.container.as_ref().filter(|c| !c.is_empty()) {
                d.insert("in".into(), json!(c));
            }
            els.push(Value::Object(d));
        }
        let mut out = Map::new();
        out.insert("app".into(), json!(a.name));
        out.insert("bundle_id".into(), json!(a.bundle_id));
        out.insert("window".into(), json!(tree.title));
        out.insert("elements".into(), Value::from(els));
        out.insert("text".into(), json!(tree.text));
        out.insert("offset".into(), json!(offset));
        out.insert("next_offset".into(), json!(tree.next_offset));
        out.insert("profile".into(), profile);
        out.insert("repeated_ax_nodes".into(), json!(tree.repeated_nodes));
        if let Some(next) = tree.next_offset {
            let mut c = Map::new();
            c.insert("offset".into(), json!(next));
            if let Some(sn) = &subtree_node {
                let root = format!("s{gen}e0");
                s.nodes.push((root.clone(), sn.clone()));
                c.insert("subtree".into(), json!(root));
            } else if let Some(w) = window.filter(|w| !w.is_empty()) {
                c.insert("window".into(), json!(w));
            }
            out.insert("continuation".into(), Value::Object(c));
        }
        if !tree.windows.is_empty() {
            out.insert("windows".into(), json!(tree.windows));
        }
        if !tree.headings.is_empty() {
            out.insert("headings".into(), json!(tree.headings));
        }
        if !tree.containers.is_empty() {
            let mut open: Vec<String> = tree.containers.clone();
            open.sort();
            open.dedup();
            out.insert("open".into(), json!(open));
        }
        if tree.truncated {
            out.insert(
                "truncated".into(),
                if tree.truncated_by.is_empty() {
                    json!(true)
                } else {
                    json!(tree.truncated_by)
                },
            );
            out.insert("retrieval".into(), json!("mac.snapshot with returned continuation arguments continues this exact page/subtree; subtree=<element id> retrieves below the depth limit; full text values are not clipped"));
        }
        Ok((a, out))
    }

    /// driver.py:732: an element of this app's latest snapshot.
    pub fn node(&self, bundle_id: &str, element: &str) -> Result<AxNode> {
        let st = self.state.lock().expect("lock");
        let s = st.get(bundle_id);
        let n = s.and_then(|s| s.node(element));
        let prefix = s
            .map(|s| format!("s{}e", s.gen))
            .unwrap_or_else(|| "-".into());
        match n {
            Some(n) if element.starts_with(&prefix) => Ok(n.clone()),
            _ => stale(format!(
                "element {element} is from an older snapshot of this app; take a new one"
            )),
        }
    }

    /// driver.py:739 (HAND-220).
    pub fn element_identity(&self, bundle_id: &str, element: &str) -> Option<Map<String, Value>> {
        let st = self.state.lock().expect("lock");
        let s = st.get(bundle_id)?;
        let n = s.node(element)?;
        if !element.starts_with(&format!("s{}e", s.gen)) {
            return None;
        }
        let mut out = Map::new();
        out.insert("window".into(), s.window_id.clone().unwrap_or(Value::Null));
        out.extend(n.identity());
        Some(out)
    }

    /// driver.py:749 (HAND-220).
    pub fn click_identity(
        &self,
        bundle_id: &str,
        shot_id: &str,
        x: f64,
        y: f64,
    ) -> Option<Map<String, Value>> {
        let st = self.state.lock().expect("lock");
        let shot = st.get(bundle_id)?.shot.as_ref()?;
        if shot.id != shot_id
            || !(0.0 <= x && x < shot.size.0 as f64 && 0.0 <= y && y < shot.size.1 as f64)
        {
            return None;
        }
        let (px, py) = shot_to_screen(shot, x, y);
        let mut out = Map::new();
        out.insert("window".into(), json!(shot.window));
        match node_at(&shot.nodes, px, py) {
            Some(n) if n.rect.2 <= CLICK_ELEMENT_MAX && n.rect.3 <= CLICK_ELEMENT_MAX => {
                out.extend(n.identity())
            }
            _ => {
                out.insert(
                    "point".into(),
                    json!([
                        ((px - shot.bounds.0) / CLICK_GRID).round_ties_even() as i64,
                        ((py - shot.bounds.1) / CLICK_GRID).round_ties_even() as i64
                    ]),
                );
            }
        }
        Some(out)
    }

    /// The app's latest shot (tests, the tools).
    pub fn latest_shot(&self, bundle_id: &str) -> Option<Shot> {
        self.state
            .lock()
            .expect("lock")
            .get(bundle_id)
            .and_then(|s| s.shot.clone())
    }

    // ---------- screenshots ----------

    fn title_now(&self, w: &Window) -> String {
        self.shots.window(w.id).map(|c| c.title).unwrap_or_default()
    }

    async fn look(
        self: &Arc<Self>,
        a: &A,
        w: &Window,
        before: Option<&Look>,
        keep: bool,
        budget_s: Option<f64>,
        masks_only: bool,
    ) -> Look {
        let earlier = before.and_then(|b| b.1.clone());
        let walk = {
            let (d, a, w) = (self.clone(), a.clone(), w.clone());
            async move {
                let (pid, bounds, title) = (a.pid, w.bounds, w.title.clone());
                let w2 = w.clone();
                let win = d
                    .blk(move |d| {
                        let now = || d.title_now(&w2);
                        d.ax.window_with_frame(
                            pid,
                            bounds,
                            if title.is_empty() { None } else { Some(&title) },
                            Some(&now),
                        )
                    })
                    .await;
                match win {
                    Some(win) => Some(d.walk(&a, Some(win), 0, keep, budget_s, masks_only).await),
                    None => None,
                }
            }
        };
        let browser = if self.is_browser(&a.bundle_id) {
            Some(crate::apps::pair_key(&a.bundle_id))
        } else {
            None
        };
        let ask = match (&browser, &self.web_secrets) {
            (Some(key), Some(ws)) if !earlier.as_ref().is_some_and(|e| e.late) => {
                Some(ws(key.clone(), w.bounds, w.title.clone()))
            }
            _ => None,
        };
        let Some(ask) = ask else {
            return (walk.await, earlier);
        };
        let (tree, mut web) = tokio::join!(walk, ask);
        if let Some(wv) = &web {
            if wv.state.is_some() && !w.title.is_empty() {
                if let Some(tab) = &wv.tab {
                    let w2 = w.clone();
                    let now_title = self.blk(move |d| d.title_now(&w2)).await;
                    if !mask::same_title(&tab.1, &w.title) && !mask::same_title(&tab.1, &now_title)
                    {
                        web = Some(WebSecrets {
                            state: None,
                            why: mask::OTHER_PAGE.into(),
                            late: false,
                            tab: None,
                        });
                    }
                }
            }
        }
        (tree, web)
    }

    /// driver.py:808 (HAND-518): one capture between its two looks; the look before walks while the shot is taken.
    async fn between_looks<G, F>(
        self: &Arc<Self>,
        a: &A,
        w: &Window,
        grab: G,
        whole_after: bool,
    ) -> Result<(Look, (Vec<u8>, i64, i64), Look)>
    where
        G: FnOnce() -> F,
        F: Future<Output = Result<(Vec<u8>, i64, i64)>>,
    {
        let ctx = preview();
        let left = preview_left(self.now());
        let first = {
            let (d, a, w) = (self.clone(), a.clone(), w.clone());
            let fut = async move {
                let now = || d.now();
                preview_phase(
                    "walk1",
                    &now,
                    d.look(&a, &w, None, false, walk_budget(left, 2.0), true),
                )
                .await
            };
            match ctx.clone() {
                Some(c) => tokio::spawn(PREVIEW.scope(c, fut)),
                None => tokio::spawn(fut),
            }
        };
        tokio::task::yield_now().await;
        let shot = match grab().await {
            Ok(s) => s,
            Err(e) => {
                first.abort();
                let _ = first.await;
                return Err(e);
            }
        };
        let before = match first.await {
            Ok(l) => l,
            Err(e) => return Err(HandError::Other(e.to_string())),
        };
        let left = preview_left(self.now());
        let now = || self.now();
        let after = preview_phase(
            "walk2",
            &now,
            self.look(
                a,
                w,
                Some(&before),
                whole_after,
                walk_budget(left, 1.0),
                !whole_after,
            ),
        )
        .await;
        Ok((before, shot, after))
    }

    /// driver.py:903: a masked capture of the app's window.
    #[allow(clippy::type_complexity)]
    async fn capture(
        self: &Arc<Self>,
        name: &str,
        window: Option<&str>,
        native_resolution: bool,
        worker: bool,
        paint: &dyn Fn(Vec<u8>, &[Rect], Rect, (i64, i64)) -> Result<Vec<u8>>,
    ) -> Result<(
        A,
        Window,
        Vec<u8>,
        (i64, i64),
        String,
        Option<(String, String)>,
        Option<AxTree>,
        Vec<Rect>,
    )> {
        let a = self.resolve(name).await?;
        self.expose(&a).await;
        let b = a.bundle_id.clone();
        let browser = self.blk(move |d| d.browser_now(&b)).await;
        let (a2, win) = (a.clone(), window.map(String::from));
        let w = self
            .blk(move |d| -> Result<Window> {
                let w = match win.as_deref() {
                    Some(wid) if !wid.is_empty() && wid.chars().all(|c| c.is_ascii_digit()) => {
                        d.shots.window(wid.parse().unwrap_or(-1)).filter(|w| w.pid == a2.pid)
                    }
                    _ => {
                        let axwin = match win.as_deref().filter(|w| !w.is_empty()) {
                            Some(wn) => d.ax.window_named(a2.pid, wn),
                            None => d.ax.app_window(a2.pid),
                        };
                        if win.as_deref().is_some_and(|w| !w.is_empty()) && axwin.is_none() {
                            return refused("no unique reachable window matches that exact title; choose a window id");
                        }
                        let like = axwin.as_ref().and_then(|x| d.ax.frame(x));
                        d.shots.front_window(a2.pid, like, None)
                    }
                };
                w.ok_or_else(|| HandError::NoTarget(format!("{} has no unique capture target; choose an exact window id from mac.snapshot", a2.name)))
            })
            .await?;
        let in_preview = preview().is_some();
        let grab = || {
            let (d, w, name) = (self.clone(), w.clone(), a.name.clone());
            async move {
                off(move || {
                    d.shots
                        .capture(
                            &w,
                            if native_resolution {
                                None
                            } else {
                                Some(crate::shots::MAX_EDGE)
                            },
                            in_preview,
                        )
                        .map_err(|e| match e {
                            HandError::NoTarget(m) => HandError::NoTarget(format!(
                                "couldn't capture {name}'s window: {m}"
                            )),
                            HandError::Capture { message, .. }
                            | HandError::Timeout(message)
                            | HandError::Other(message) => HandError::Refused(format!(
                                "couldn't capture {name}'s window: {message}"
                            )),
                            other => other,
                        })
                })
                .await
            }
        };
        let (first, (jpeg, width, height), second) =
            self.between_looks(&a, &w, grab, worker).await?;
        let (masks, note) = mask::window_masks(&first, &second, browser);
        let image = paint(jpeg, &masks, w.bounds, (width, height))?;
        let tab = if browser {
            let w2 = w.clone();
            let now_title = self.blk(move |d| d.title_now(&w2)).await;
            mask::page_tab(&[w.title.clone(), now_title], &[&first, &second])
        } else {
            None
        };
        Ok((
            a,
            w,
            image,
            (width, height),
            note,
            tab,
            if worker { second.0 } else { None },
            masks,
        ))
    }

    /// driver.py:831: mac.screenshot; the capture becomes the app's latest shot.
    pub async fn screenshot(
        self: &Arc<Self>,
        name: &str,
        window: Option<&str>,
        native_resolution: bool,
        paint: &dyn Fn(Vec<u8>, &[Rect], Rect, (i64, i64)) -> Result<Vec<u8>>,
    ) -> Result<(A, Shot, Vec<u8>)> {
        let (a, w, jpeg, size, note, tab, tree, masks) = self
            .capture(name, window, native_resolution, true, paint)
            .await?;
        let at = self.now();
        let mut st = self.state.lock().expect("lock");
        let s = st.entry(a.bundle_id.clone()).or_default();
        s.shots += 1;
        let shot = Shot {
            id: format!("shot{}", s.shots),
            window: w.id,
            bounds: w.bounds,
            size,
            pid: a.pid,
            at,
            signature: tree.as_ref().map(|t| t.signature()),
            masking: note,
            tab,
            nodes: tree.map(|t| t.nodes).unwrap_or_default(),
            title: w.title.clone(),
            masks,
        };
        s.shot = Some(shot.clone());
        drop(st);
        Ok((a, shot, jpeg))
    }

    /// driver.py:849: FERB's own picture of the window for the evidence store; never the app's latest shot.
    pub async fn window_image(
        self: &Arc<Self>,
        name: &str,
        window: Option<&str>,
        paint: &dyn Fn(Vec<u8>, &[Rect], Rect, (i64, i64)) -> Result<Vec<u8>>,
    ) -> Result<(A, Shot, Vec<u8>)> {
        let (a, w, jpeg, size, note, tab, _, masks) =
            self.capture(name, window, false, false, paint).await?;
        let shot = Shot {
            id: "evidence".into(),
            window: w.id,
            bounds: w.bounds,
            size,
            pid: a.pid,
            at: self.now(),
            signature: None,
            masking: note,
            tab,
            nodes: vec![],
            title: w.title.clone(),
            masks,
        };
        Ok((a, shot, jpeg))
    }

    /// driver.py:859 (B6).
    pub async fn page_secrets(
        self: &Arc<Self>,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> Result<Option<(Vec<Rect>, Rect, bool)>> {
        let a = self.resolve(&format!("pid:{pid}")).await?;
        let t = title.to_string();
        let win = self
            .blk(move |d| {
                let win = d.ax.window_with_frame(
                    pid,
                    bounds,
                    if t.is_empty() { None } else { Some(&t) },
                    None,
                )?;
                if !t.is_empty() && !mask::same_title(&d.ax.window_title(&win), &t) {
                    return None;
                }
                Some(win)
            })
            .await;
        let Some(win) = win else { return Ok(None) };
        let tree = self.walk(&a, Some(win), 0, false, None, true).await;
        let areas = ax::outermost(&tree.web_areas);
        if tree.secure.is_none() || areas.len() != 1 || areas[0].2 <= 0.0 || areas[0].3 <= 0.0 {
            return Ok(None);
        }
        let (boxes, whole) = mask::ax_boxes(&tree);
        let (ax_, ay, aw, ah) = areas[0];
        let inside = boxes
            .into_iter()
            .filter(|b| b.0 < ax_ + aw && b.0 + b.2 > ax_ && b.1 < ay + ah && b.1 + b.3 > ay)
            .collect();
        Ok(Some((inside, areas[0], whole)))
    }

    /// driver.py:879 (L33): a page element's rect in screen points, or None whenever unsure.
    pub fn place_on_web_area(
        &self,
        rect: Rect,
        viewport: Rect,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> Option<Rect> {
        self.app(&format!("pid:{pid}")).ok()?;
        let t = if title.is_empty() { None } else { Some(title) };
        let win = self.ax.window_with_frame(pid, bounds, t, None)?;
        if !title.is_empty() && !mask::same_title(&self.ax.window_title(&win), title) {
            return None;
        }
        let areas = ax::outermost(&self.ax.web_areas_of(&win).unwrap_or_default());
        let placed = mask::page_on_screen(&[rect], viewport, &areas).ok()?;
        if placed.len() == 1
            && self
                .shots
                .window_on_screen(pid, placed[0], Some(bounds), t, None)
        {
            Some(placed[0])
        } else {
            None
        }
    }

    /// driver.py:948: pin the window the worker saw for a recording.
    pub fn recording_source(
        self: &Arc<Self>,
        name: &str,
        shot_id: &str,
        paint: PaintFn,
    ) -> Result<(Map<String, Value>, FrameFn)> {
        let a = self.app(name)?;
        let shot = self.latest_shot(&a.bundle_id);
        match shot {
            Some(s) if s.id == shot_id && s.pid == a.pid && self.now() - s.at <= 30.0 => {
                Ok(self.window_source(a, s.window, paint))
            }
            _ => refused("take a fresh mac_screenshot to select the recording window"),
        }
    }

    /// driver.py:960: only an explicit window or an observed element's exact window.
    pub fn preview_source(
        self: &Arc<Self>,
        name: &str,
        window: Option<&str>,
        element: Option<&str>,
        paint: PaintFn,
    ) -> Result<(Map<String, Value>, FrameFn)> {
        let a = self.app(name)?;
        let w = match window {
            Some(wid) if !wid.is_empty() && wid.chars().all(|c| c.is_ascii_digit()) => {
                self.shots.window(wid.parse().unwrap_or(-1))
            }
            _ => {
                let win = if let Some(e) = element.filter(|e| !e.is_empty()) {
                    self.ax.element_window(&self.node(&a.bundle_id, e)?.r#ref)
                } else if let Some(wn) = window.filter(|w| !w.is_empty()) {
                    self.ax.window_named(a.pid, wn)
                } else {
                    None
                };
                let Some(bounds) = win.as_ref().and_then(|w| self.ax.frame(w)) else {
                    return refused("preview needs an exact observed window");
                };
                self.shots.front_window(a.pid, Some(bounds), None)
            }
        };
        match w {
            Some(w) if w.pid == a.pid => Ok(self.window_source(a, w.id, paint)),
            _ => refused("preview window closed or ambiguous"),
        }
    }

    fn window_source(
        self: &Arc<Self>,
        a: A,
        window_id: i64,
        paint: PaintFn,
    ) -> (Map<String, Value>, FrameFn) {
        let mut target = Map::new();
        target.insert("kind".into(), json!("window"));
        target.insert("app".into(), json!(a.bundle_id));
        target.insert("pid".into(), json!(a.pid));
        target.insert("window_id".into(), json!(window_id));
        let d = self.clone();
        let capture: FrameFn = Arc::new(move || {
            let (d, a, paint) = (d.clone(), a.clone(), paint.clone());
            Box::pin(async move {
                let current = {
                    let (d, a) = (d.clone(), a.clone());
                    move || {
                        let (d, a) = (d.clone(), a.clone());
                        async move {
                            let w = d.blk(move |d| d.shots.window(window_id)).await;
                            match w {
                                Some(w)
                                    if w.pid == a.pid
                                        && d.resolve(&a.bundle_id).await?.pid == a.pid =>
                                {
                                    Ok(w)
                                }
                                _ => refused("recording window closed or changed owner"),
                            }
                        }
                    }
                };
                let now = || d.now();
                let w = preview_phase("resolve", &now, async {
                    let w = current().await?;
                    d.expose(&a).await;
                    Ok::<Window, HandError>(w)
                })
                .await?;
                let browser = d.is_browser(&a.bundle_id);
                let in_preview = preview().is_some();
                let grab = || {
                    let (d2, w2, cur) = (d.clone(), w.clone(), current.clone());
                    async move {
                        let shot = {
                            let d3 = d2.clone();
                            off(move || {
                                d3.shots
                                    .capture(&w2, Some(crate::shots::MAX_EDGE), in_preview)
                            })
                            .await?
                        };
                        cur().await?;
                        Ok(shot)
                    }
                };
                let (first, (image, width, height), second) =
                    d.between_looks(&a, &w, grab, false).await?;
                let (masks, note) = mask::window_masks(&first, &second, browser);
                let frame = preview_phase("paint", &now, async {
                    paint(image, &masks, w.bounds, (width, height))
                })
                .await?;
                Ok::<Frame, HandError>((frame, note))
            })
        });
        (target, capture)
    }

    // ---------- pointer input ----------

    /// driver.py:1008: exact visual input to an app process; never activates or takes over the global mouse.
    #[allow(clippy::too_many_arguments)]
    pub async fn pointer_at(
        self: &Arc<Self>,
        name: &str,
        shot_id: &str,
        x: f64,
        y: f64,
        kind: &str,
        end: Option<(f64, f64)>,
        holder: &str,
        direction: &str,
        amount: i64,
        distance: f64,
        terminal: Option<&TerminalGrant>,
        task: Option<&str>,
    ) -> Result<ActionResult> {
        let a = self.resolve(name).await?;
        if kind != "hover" {
            self.armed.lock().expect("lock").remove(&a.pid);
        }
        {
            let (a2, s, k, g, t) = (
                a.clone(),
                shot_id.to_string(),
                kind.to_string(),
                terminal.cloned(),
                task.map(String::from),
            );
            if let Some(r) = self
                .blk(move |d| d.pointer_refusal(&a2, &s, x, y, &k, end, g.as_ref(), t.as_deref()))
                .await
            {
                return refused(r);
            }
        }
        let shot = self.latest_shot(&a.bundle_id);
        let shot = match shot {
            Some(s) if s.id == shot_id && self.now() - s.at <= SHOT_AGE_S => s,
            _ => return stale("take a fresh screenshot before pointer input"),
        };
        if !["click", "dblclick", "rightclick", "hover", "drag", "scroll"].contains(&kind) {
            return refused("unsupported pointer action");
        }
        if !["up", "down", "left", "right"].contains(&direction) || !(1..=50).contains(&amount) {
            return refused("scroll needs a direction and amount between 1 and 50");
        }
        if !distance.is_finite() || !(0.01..=1.0).contains(&distance) {
            return refused("scroll distance must be a viewport fraction from 0.01 to 1.0");
        }
        let route = self.route;
        if kind == "drag" && end.is_none() {
            return refused("drag needs an end point");
        }
        if kind == "click" || kind == "dblclick" {
            let (a2, s) = (a.clone(), shot_id.to_string());
            let combo = self
                .blk(move |d| d.item_combo(d.hit_in_shot(&a2, &s, x, y).as_ref()))
                .await;
            if let Some(combo) = combo {
                if let Some(handled) = self.guarded_combo(&a, &combo, task).await? {
                    return Ok(handled);
                }
            }
        }
        let points = [(x, y), end.unwrap_or((x, y))];
        let inside = |p: &(f64, f64)| {
            p.0.is_finite()
                && p.1.is_finite()
                && 0.0 <= p.0
                && p.0 < shot.size.0 as f64
                && 0.0 <= p.1
                && p.1 < shot.size.1 as f64
        };
        if !points.iter().all(inside) {
            return refused("pointer point outside the screenshot");
        }
        let start = shot_to_screen(&shot, points[0].0, points[0].1);
        let finish = shot_to_screen(&shot, points[1].0, points[1].1);
        let pid = a.pid;
        if self.blk(move |d| d.own_pid_is(pid)).await {
            return refused(OWN_APP);
        }
        let window_ok = {
            let (d, a, shot) = (self.clone(), a.clone(), shot.clone());
            Arc::new(move || -> Result<()> {
                let w = d.shots.window(shot.window);
                match w {
                    Some(w) if w.pid == a.pid && same_rect(&w.bounds, &shot.bounds) => {}
                    _ => return stale(WINDOW_GONE),
                }
                if !route && !d.shots.on_screen(shot.window) {
                    return refused("window-targeted backend unavailable; the legacy pointer route is limited to the current Space on this Mac. No foreground switch was made");
                }
                let front = d.apps.frontmost();
                if (d.input.owner_active)() && front.is_some_and(|f| f.pid == a.pid) {
                    return refused(
                        "the owner is (or may be) using this app; pointer input stopped",
                    );
                }
                Ok(())
            })
        };
        let facts = {
            let (a2, s2, wo) = (a.clone(), shot.clone(), window_ok.clone());
            self.blk(move |d| -> Result<ActionFacts> {
                wo()?;
                if let Some(why) = d.changed_at(&a2, &s2, start.0, start.1) {
                    return stale(why);
                }
                Ok(d.point_facts(&a2, &s2, start.0, start.1))
            })
            .await?
        };
        if kind == "scroll" {
            let r = self
                .pointer_scroll(
                    &a, &shot, start, direction, amount, holder, route, window_ok, distance,
                )
                .await?;
            return Ok(ActionResult {
                observed: Some(facts),
                ..r
            });
        }
        let (steps, release) = self.gestures.pointer_steps(
            start,
            finish,
            kind,
            a.pid,
            Some(shot.window as u32),
            route,
            (shot.bounds.0, shot.bounds.1),
        )?;
        let attempted = Arc::new(Mutex::new(false));
        let steps: Vec<Step> = steps
            .into_iter()
            .map(|s| {
                let (wo, at) = (window_ok.clone(), attempted.clone());
                s.guarded(move || {
                    wo()?;
                    *at.lock().expect("lock") = true;
                    Ok(())
                })
            })
            .collect();
        if let Some(s) = self.state.lock().expect("lock").get_mut(&a.bundle_id) {
            s.shot = None;
        }
        if let Err(e) = self
            .input
            .to_process(steps, holder, Some(release), None)
            .await
        {
            if *attempted.lock().expect("lock")
                && matches!(e, HandError::Refused(_) | HandError::StaleRef(_))
            {
                return Err(HandError::PointerInterrupted(format!(
                    "pointer input interrupted; outcome unknown: {e}"
                )));
            }
            return Err(e);
        }
        self.sleep(SETTLE_S).await;
        let (a2, s2) = (a.clone(), shot.clone());
        let now = self.blk(move |d| d.window_signature(&a2, &s2)).await;
        let changed = now != shot.signature;
        if kind != "hover" {
            self.profiles
                .record(&a.bundle_id, &format!("pid_pointer_{kind}"), changed);
        }
        Ok(ActionResult {
            ok: true,
            detail: format!(
                "{kind} input delivered to the app process; visible state {}; verify with a fresh screenshot",
                if changed { "changed" } else { "not confirmed changed" }
            ),
            facts: json!({"method": if route { "window_pointer" } else { "pid_pointer" }, "delivered": true,
                          "observed_change": changed, "outcome_verified": false, "replayed": false})
            .as_object()
            .cloned()
            .unwrap_or_default(),
            observed: Some(facts),
        })
    }

    /// driver.py:1095: why the screenshot's window no longer shows what the worker saw at this point.
    fn changed_at(&self, a: &A, shot: &Shot, px: f64, py: f64) -> Option<String> {
        let then = node_at(&shot.nodes, px, py);
        let now = if then.is_some() {
            self.ax.hit_element(a.pid, px, py)
        } else {
            None
        };
        if let (Some(then), Some(now)) = (then, now.as_ref()) {
            if self.holds(then, now) {
                return None;
            }
            return Some("what is under that point changed since the screenshot (another control is there now); take another screenshot".into());
        }
        if shot.signature.is_none() || self.window_signature(a, shot) != shot.signature {
            return Some("the app changed since the screenshot; take another screenshot".into());
        }
        None
    }

    fn holds(&self, then: &AxNode, now: &El) -> bool {
        let mut el = Some(now.clone());
        for _ in 0..8 {
            let Some(e) = el else { break };
            if self.ax.same(&e, &then.r#ref) {
                return true;
            }
            el = self.ax.parent_of(&e);
        }
        let rect = self.ax.frame(now);
        !then.ax_role.is_empty()
            && self.ax.role_of(now) == then.ax_role
            && rect.is_some_and(|r| same_rect(&r, &then.rect))
    }

    fn point_facts(&self, a: &A, shot: &Shot, px: f64, py: f64) -> ActionFacts {
        let el = self.ax.hit_element(a.pid, px, py);
        let mut f = self.element_facts(a, el.as_ref());
        let window = if shot.title.is_empty() {
            shot.window.to_string()
        } else {
            shot.title.clone()
        };
        f.received_by = Some(ReceivedBy {
            bundle_id: Some(a.bundle_id.clone()),
            window: Some(window),
            role: f.target_role.clone(),
        });
        f
    }

    /// driver.py:1141 (L33).
    pub fn shot_point(
        &self,
        name: &str,
        shot_id: &str,
        x: f64,
        y: f64,
    ) -> Result<Option<(i32, i64, (f64, f64))>> {
        let a = self.app(name)?;
        let Some(shot) = self.latest_shot(&a.bundle_id) else {
            return Ok(None);
        };
        if shot.id != shot_id
            || !(0.0 <= x && x < shot.size.0 as f64 && 0.0 <= y && y < shot.size.1 as f64)
            || self.now() - shot.at > SHOT_AGE_S
        {
            return Ok(None);
        }
        match self.shots.window(shot.window) {
            Some(now) if now.pid == a.pid && same_rect(&now.bounds, &shot.bounds) => {
                Ok(Some((a.pid, shot.window, shot_to_screen(&shot, x, y))))
            }
            _ => Ok(None),
        }
    }

    /// driver.py:1157: one explicit pointer, AX or foreground method for the captured window.
    #[allow(clippy::too_many_arguments)]
    pub async fn click_at(
        self: &Arc<Self>,
        name: &str,
        shot_id: &str,
        x: f64,
        y: f64,
        holder: &str,
        task: Option<&str>,
        allow_foreground: bool,
        mechanism: &str,
        terminal: Option<&TerminalGrant>,
    ) -> Result<ActionResult> {
        let a = self.resolve(name).await?;
        self.armed.lock().expect("lock").remove(&a.pid);
        {
            let (a2, s, g, t) = (
                a.clone(),
                shot_id.to_string(),
                terminal.cloned(),
                task.map(String::from),
            );
            if let Some(r) = self
                .blk(move |d| {
                    d.pointer_refusal(&a2, &s, x, y, "click", None, g.as_ref(), t.as_deref())
                })
                .await
            {
                return refused(r);
            }
        }
        let shot = match self.latest_shot(&a.bundle_id) {
            Some(s) if s.id == shot_id => s,
            _ => {
                return stale(format!(
                    "{shot_id} isn't {}'s latest screenshot; take a new one",
                    a.name
                ))
            }
        };
        if self.now() - shot.at > SHOT_AGE_S {
            return stale(format!(
                "{shot_id} is more than {:.0} s old; take a new screenshot",
                SHOT_AGE_S
            ));
        }
        if !(0.0 <= x && x < shot.size.0 as f64 && 0.0 <= y && y < shot.size.1 as f64) {
            return refused(format!(
                "({}, {}) is outside the {}x{} screenshot",
                ferb_py::json::python_float(x),
                ferb_py::json::python_float(y),
                shot.size.0,
                shot.size.1
            ));
        }
        {
            let (a2, s) = (a.clone(), shot_id.to_string());
            let combo = self
                .blk(move |d| d.item_combo(d.hit_in_shot(&a2, &s, x, y).as_ref()))
                .await;
            if let Some(combo) = combo {
                if let Some(h) = self.guarded_combo(&a, &combo, task).await? {
                    return Ok(h);
                }
            }
        }
        if mechanism == "pointer" {
            return self
                .pointer_at(
                    name, shot_id, x, y, "click", None, holder, "down", 1, 0.8, terminal, task,
                )
                .await;
        }
        if mechanism != "ax" && mechanism != "foreground" {
            return refused("click mechanism must be pointer, ax or foreground");
        }
        let wid = shot.window;
        let now = self.blk(move |d| d.shots.window(wid)).await;
        match now {
            Some(n) if n.pid == a.pid && same_rect(&n.bounds, &shot.bounds) => {}
            _ => return stale("the window moved, resized, changed owner or closed since the screenshot; take a new one"),
        }
        let (px, py) = shot_to_screen(&shot, x, y);
        if mechanism == "foreground" {
            let pid = a.pid;
            if let Some(why) = self
                .blk(move |d| d.own_point(px, py, Some(pid), Some(wid)))
                .await
            {
                return refused(why);
            }
        }
        let facts = {
            let (a2, s2) = (a.clone(), shot.clone());
            self.blk(move |d| d.point_facts(&a2, &s2, px, py)).await
        };
        if mechanism == "foreground" {
            if !allow_foreground {
                return Ok(ok_result(
                    false,
                    format!(
                        "no control under ({}, {}) could be pressed",
                        fmt0(x),
                        fmt0(y)
                    ),
                    json!({"code": "needs_foreground", "performed": false}),
                ));
            }
            let steps = self.gestures.click_steps(px, py, None);
            let r = self
                .foreground_at(
                    &a,
                    px,
                    py,
                    steps,
                    shot.bounds,
                    shot.window,
                    holder,
                    "click",
                    task,
                )
                .await?;
            return Ok(ActionResult {
                ok: true,
                detail: format!(
                    "clicked ({}, {}) in {}; {}",
                    fmt0(x),
                    fmt0(y),
                    shot.id,
                    r.detail
                ),
                facts: r.facts,
                observed: Some(facts),
            });
        }
        let pid = a.pid;
        let el = self.blk(move |d| d.ax.hit_element(pid, px, py)).await;
        let Some(el) = el else {
            return Ok(ActionResult {
                observed: Some(facts),
                ..ok_result(
                    false,
                    "AX hit test unavailable; no pointer input attempted",
                    json!({"method": "ax", "performed": false}),
                )
            });
        };
        let e2 = el.clone();
        let win_frame = self
            .blk(move |d| d.ax.element_window(&e2).and_then(|w| d.ax.frame(&w)))
            .await;
        if !win_frame.is_some_and(|f| same_rect(&f, &shot.bounds)) {
            return Ok(ActionResult {
                observed: Some(facts),
                ..ok_result(
                    false,
                    "AX hit target cannot be bound to the captured window",
                    json!({"method": "ax", "performed": false}),
                )
            });
        }
        let ok = self.blk(move |d| d.ax.press(&el)).await?;
        Ok(ActionResult {
            observed: Some(facts),
            ..ok_result(
                ok,
                if ok {
                    "AX press accepted; inspect the result"
                } else {
                    "AX press refused"
                },
                json!({"method": "ax", "accepted": ok, "outcome_verified": false}),
            )
        })
    }

    // ---------- the foreground, last resort, only by request ----------

    #[allow(clippy::too_many_arguments)]
    async fn foreground_at(
        self: &Arc<Self>,
        a: &A,
        x: f64,
        y: f64,
        steps: Vec<Step>,
        bounds: Rect,
        window: i64,
        holder: &str,
        what: &str,
        task: Option<&str>,
    ) -> Result<ActionResult> {
        let pid = a.pid;
        let guarded: Vec<Step> = steps
            .into_iter()
            .map(|s| {
                let d = self.clone();
                s.guarded(move || {
                    if let Some(why) = d.own_point(x, y, Some(pid), Some(window)) {
                        return refused(why);
                    }
                    let w = d.shots.window(window);
                    let reaches = || match d.ax.hit(x, y) {
                        Some((p, f)) => p == pid && f.is_some_and(|f| same_rect(&f, &bounds)),
                        None => d.shots.window_at(x, y).is_some_and(|t| t.id == window),
                    };
                    if !w.is_some_and(|w| same_rect(&w.bounds, &bounds)) || !reaches() {
                        return stale("another window is over that point now, or the window moved; take a new look");
                    }
                    Ok(())
                })
            })
            .collect();
        if let Some(why) = self
            .blk(move |d| d.own_point(x, y, Some(pid), Some(window)))
            .await
        {
            return refused(why);
        }
        let (d, a2) = (self.clone(), a.clone());
        self.real(
            a,
            guarded,
            holder,
            what,
            task,
            Some(Box::pin(async move {
                d.bring_forward(&a2, bounds, window).await
            })),
        )
        .await
    }

    fn element_center(&self, el: &El, fallback: (f64, f64)) -> (f64, f64) {
        match self.ax.frame(el) {
            Some((x, y, w, h)) => (x + w / 2.0, y + h / 2.0),
            None => fallback,
        }
    }

    fn element_window_of(&self, a: &A, el: &El) -> Result<(Rect, i64)> {
        let win = self.ax.element_window(el);
        let bounds = win.as_ref().and_then(|w| self.ax.frame(w));
        let title = win
            .as_ref()
            .map(|w| self.ax.window_title(w))
            .unwrap_or_default();
        let found = bounds.and_then(|b| {
            self.shots.front_window(
                a.pid,
                Some(b),
                if title.is_empty() { None } else { Some(&title) },
            )
        });
        match (bounds, found) {
            (Some(b), Some(f)) => Ok((b, f.id)),
            _ => refused("FERB can't tell which window holds that control, so it brought nothing forward; take a new snapshot, or use mac.screenshot and mac.click"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn foreground_element(
        self: &Arc<Self>,
        a: &A,
        el: &El,
        fallback: (f64, f64),
        steps: impl Fn(f64, f64) -> Vec<Step>,
        holder: &str,
        what: &str,
        task: Option<&str>,
    ) -> Result<ActionResult> {
        let e = el.clone();
        let (x, y) = self.blk(move |d| d.element_center(&e, fallback)).await;
        let pid = a.pid;
        if let Some(why) = self.blk(move |d| d.own_point(x, y, Some(pid), None)).await {
            return refused(why);
        }
        let (a2, e) = (a.clone(), el.clone());
        let (bounds, window) = self.blk(move |d| d.element_window_of(&a2, &e)).await?;
        self.foreground_at(a, x, y, steps(x, y), bounds, window, holder, what, task)
            .await
    }

    async fn bring_forward(self: &Arc<Self>, a: &A, bounds: Rect, window: i64) -> Result<()> {
        let pid = a.pid;
        self.blk(move |d| d.ax.raise_window(pid, bounds)).await;
        for _ in 0..12 {
            let front = self.blk(|d| d.apps.frontmost()).await;
            if (self.input.owner_active)() || !front.is_some_and(|f| f.pid == a.pid) {
                return refused("owner input or focus changed while waiting for the explicitly requested window");
            }
            if self.blk(move |d| d.shots.on_screen(window)).await {
                self.sleep(0.2).await;
                return Ok(());
            }
            self.sleep(0.25).await;
        }
        Ok(())
    }

    /// driver.py:2498: an explicitly chosen foreground action; yields if the owner takes over; restores nothing.
    async fn real(
        self: &Arc<Self>,
        a: &A,
        steps: Vec<Step>,
        holder: &str,
        what: &str,
        task: Option<&str>,
        prepare: Option<Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>>,
    ) -> Result<ActionResult> {
        if !(self.input.available)() {
            return Ok(ok_result(
                false,
                (self.input.why_unavailable)(),
                json!({"performed": false}),
            ));
        }
        self.input.quiet(holder, what).await?;
        let front = self.blk(|d| d.apps.frontmost()).await;
        if !front.is_some_and(|f| f.pid == a.pid) {
            (self.on_activate)(&a.bundle_id, task);
            let (b, p) = (a.bundle_id.clone(), a.pid);
            self.blk(move |d| d.apps.activate(&b, Some(p))).await;
            self.sleep(0.3).await;
        }
        if let Some(p) = prepare {
            p.await?;
        }
        let pid = a.pid;
        let guarded: Vec<Step> = steps
            .into_iter()
            .map(|s| {
                let d = self.clone();
                s.guarded(move || {
                    if !d.apps.frontmost().is_some_and(|f| f.pid == pid) {
                        return Err(HandError::PointerInterrupted(
                            "foreground target changed; input stopped without restoring an app"
                                .into(),
                        ));
                    }
                    Ok(())
                })
            })
            .collect();
        let n = self.input.run(guarded, holder, what).await?;
        Ok(ok_result(
            true,
            format!("{what} ({n} steps, explicit foreground input); inspect the result"),
            json!({"method": "foreground", "delivered_steps": n, "outcome_verified": false}),
        ))
    }
}

/// Paints a capture's boxes (`mask_secret_fields` through `to_image_boxes`): the JPEG, its boxes in screen points,
/// the window bounds and the image size.
pub type PaintFn = Arc<dyn Fn(Vec<u8>, &[Rect], Rect, (i64, i64)) -> Result<Vec<u8>> + Send + Sync>;

/// driver.py:255: every box painted opaque; withheld when it can't be.
pub fn painted(
    paint: &dyn crate::ocr::Paint,
    jpeg: Vec<u8>,
    masks: &[Rect],
    bounds: Rect,
    size: (i64, i64),
) -> Result<Vec<u8>> {
    let boxes = mask::to_image_boxes(masks, bounds, size);
    if boxes.is_empty() {
        return Ok(jpeg);
    }
    paint
        .paint(&jpeg, &boxes)
        .map_err(|_| HandError::Withheld(mask::UNPAINTABLE.into()))
}

pub fn file_facts(path: &str) -> (String, String, String) {
    match std::fs::read(path) {
        Err(_) => (
            path.into(),
            String::new(),
            "(a file FERB can't read)".into(),
        ),
        Ok(data) => {
            let digest = hex::encode(sha2::Sha256::digest(&data));
            let text = match String::from_utf8(data.clone()) {
                Ok(t) => t,
                Err(_) => format!("(a binary file of {} bytes)", data.len()),
            };
            (path.into(), digest, text)
        }
    }
}

mod act;
mod terminal;
