//! Accessibility reads and exact element operations (ferb/act/mac/accessibility.py), written once over an
//! [`AxSource`]: the native one (`native::ax`) reads AXUIElements, a test's reads a fake tree. Must not interpret
//! labels or choose among plausible targets.
//!
//! One walk per snapshot (`read_tree`) visits the window, any sheet in it, the app's open menus, popovers and dialogs,
//! and records the controls (anything exposing an action), the text, the headings, the masking inventory (secret
//! field frames, web areas: L13) and a signature. Password fields report filled/empty only; their value is never
//! read. A walk stops at its budget (HAND-221) and then can't tell about password fields. `masks_only` (HAND-518)
//! visits the same elements in the same order reading only what masking needs.

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde_json::{Map, Value};

pub const MAX_DEPTH: usize = 40;
pub const MAX_ELEMENTS: usize = 5000;
/// HAND-221: how long one Accessibility message waits (macOS's default is 6 s) and what one walk may read.
pub const AX_MESSAGE_TIMEOUT_S: f64 = 2.0;
pub const WALK_BUDGET_S: f64 = 8.0;
pub const WALK_MAX_NODES: usize = 60000;
pub const AX_CANNOT_COMPLETE: i32 = -25204;
pub const WEB_AREAS_BUDGET_S: f64 = 0.25;
pub const WEB_AREAS_MAX: usize = 3000;
pub const SURFACE_DEPTH: usize = 60;
pub const WINDOW_WALK_MAX: usize = 2000;
pub const ACTIVE_EDITOR_MAX: usize = 4000;

pub type Rect = (f64, f64, f64, f64);

/// accessibility.py:39.
pub fn role_kind(role: &str) -> Option<&'static str> {
    Some(match role {
        "AXButton" | "AXMenuButton" | "AXDisclosureTriangle" => "button",
        "AXLink" => "link",
        "AXTextField" | "AXTextArea" | "AXSecureTextField" => "textbox",
        "AXCheckBox" => "checkbox",
        "AXRadioButton" => "radio",
        "AXPopUpButton" => "popup",
        "AXComboBox" => "combobox",
        "AXMenuItem" | "AXMenuBarItem" => "menuitem",
        "AXSlider" => "slider",
        "AXIncrementor" => "stepper",
        "AXTab" => "tab",
        _ => return None,
    })
}

const TEXT_ROLES: [&str; 2] = ["AXStaticText", "AXHeading"];
const VALUE_ROLES: [&str; 9] = [
    "AXTextField",
    "AXTextArea",
    "AXComboBox",
    "AXCheckBox",
    "AXRadioButton",
    "AXPopUpButton",
    "AXSecureTextField",
    "AXSlider",
    "AXIncrementor",
];
const DIALOG_SUBROLES: [&str; 2] = ["AXDialog", "AXSystemDialog"];
const PASSWORD_ROLES: [&str; 3] = ["AXTextField", "AXTextArea", "AXComboBox"];
const PASSWORD_ATTRS: [&str; 5] = [
    "AXTitle",
    "AXDescription",
    "AXPlaceholderValue",
    "AXIdentifier",
    "AXDOMIdentifier",
];

fn action_name(a: &str) -> Option<&'static str> {
    Some(match a {
        "AXPress" => "press",
        "AXShowMenu" => "menu",
        "AXIncrement" => "increment",
        "AXDecrement" => "decrement",
        "AXConfirm" => "confirm",
        "AXCancel" => "cancel",
        "AXPick" => "pick",
        "AXScrollToVisible" => "scroll_to_visible",
        "AXRaise" => "raise",
        _ => return None,
    })
}

/// A text field whose metadata names it a password field: `(?i)passw|pwd` (accessibility.py:57).
pub fn password_metadata(v: &str) -> bool {
    let low = ferb_py::text::lower(v);
    // (?i) in Python folds ASCII and Unicode simple cases; the needles are ASCII, so lowercasing is enough except
    // for the Kelvin sign and long s, which fold to k and s (no needle letter is either).
    low.contains("passw") || low.contains("pwd")
}

/// An element: the native AXUIElement or a test's fake. Kept in the daemon only.
#[derive(Clone)]
pub struct El(pub Arc<dyn Any + Send + Sync>);

impl El {
    pub fn new<T: Any + Send + Sync>(v: T) -> El {
        El(Arc::new(v))
    }
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }
    /// The same Rust object (Python's `is`).
    pub fn is(&self, other: &El) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for El {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = self.get::<String>() {
            write!(f, "El({s:?})")
        } else {
            f.write_str("El(..)")
        }
    }
}

/// An attribute's value as PyObjC hands it over.
#[derive(Debug, Clone)]
pub enum AxValue {
    None,
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    Point(f64, f64),
    Size(f64, f64),
    Range(i64, i64),
    El(El),
    List(Vec<AxValue>),
    /// An NSURL: its absolute string and, for a file URL, its path.
    Url {
        absolute: String,
        path: Option<String>,
    },
}

impl AxValue {
    pub fn is_none(&self) -> bool {
        matches!(self, AxValue::None)
    }
    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            AxValue::None => false,
            AxValue::Str(s) => !s.is_empty(),
            AxValue::Bool(b) => *b,
            AxValue::Int(n) => *n != 0,
            AxValue::Float(f) => *f != 0.0,
            AxValue::List(l) => !l.is_empty(),
            _ => true,
        }
    }
    /// Python `str(v)` for the scalar kinds.
    pub fn py_str(&self) -> String {
        match self {
            AxValue::None => "None".into(),
            AxValue::Str(s) => s.clone(),
            AxValue::Bool(b) => if *b { "True" } else { "False" }.into(),
            AxValue::Int(n) => n.to_string(),
            AxValue::Float(f) => ferb_py::json::python_float(*f),
            AxValue::Url { absolute, .. } => absolute.clone(),
            AxValue::Point(x, y) => format!(
                "({}, {})",
                ferb_py::json::python_float(*x),
                ferb_py::json::python_float(*y)
            ),
            AxValue::Size(w, h) => format!(
                "({}, {})",
                ferb_py::json::python_float(*w),
                ferb_py::json::python_float(*h)
            ),
            AxValue::Range(a, b) => format!("({a}, {b})"),
            AxValue::El(_) => "<AXUIElement>".into(),
            AxValue::List(_) => "[...]".into(),
        }
    }
    /// `str(a or b)`-style: the first truthy of the two.
    pub fn or(self, other: AxValue) -> AxValue {
        if self.truthy() {
            self
        } else {
            other
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            AxValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_el(&self) -> Option<El> {
        match self {
            AxValue::El(e) => Some(e.clone()),
            _ => None,
        }
    }
    /// A list of elements (`_attr(...) or []`).
    pub fn els(&self) -> Vec<El> {
        match self {
            AxValue::List(items) => items.iter().filter_map(|v| v.as_el()).collect(),
            _ => vec![],
        }
    }
    /// Python `bool(v)` where it may be None.
    pub fn bool_or(&self, none: Option<bool>) -> Option<bool> {
        if self.is_none() {
            none
        } else {
            Some(self.truthy())
        }
    }
}

/// A window as the window server lists it (accessibility.py:293 `cg_windows`).
#[derive(Debug, Clone, PartialEq)]
pub struct CgWindow {
    pub id: i64,
    pub title: String,
    pub onscreen: bool,
    pub bounds: Rect,
}

/// The reads and writes the walk and the element operations take, on one element at a time.
pub trait AxSource: Send + Sync {
    /// The system-wide element (with FERB's messaging timeout set once).
    fn system(&self) -> El;
    /// An app's element, with FERB's messaging timeout set on it.
    fn app(&self, pid: i32) -> El;
    /// One attribute; None on any error or no value.
    fn attr(&self, el: &El, name: &str) -> AxValue;
    fn actions(&self, el: &El) -> Vec<String>;
    /// AXUIElementPerformAction's code.
    fn perform(&self, el: &El, action: &str) -> i32;
    /// AXUIElementSetAttributeValue's code.
    fn set(&self, el: &El, name: &str, value: AxValue) -> i32;
    /// AXUIElementCopyElementAtPosition on `root`.
    fn element_at(&self, root: &El, x: f64, y: f64) -> Option<El>;
    fn pid_of(&self, el: &El) -> Option<i32>;
    /// CFEqual.
    fn same(&self, a: &El, b: &El) -> bool;
    /// CFHash (a bucket key; `same` decides).
    fn hash(&self, el: &El) -> u64;
    fn cg_windows(&self, pid: i32) -> Vec<CgWindow>;
    fn monotonic(&self) -> f64;
    fn sleep(&self, seconds: f64);
}

/// One element of a snapshot (accessibility.py:63).
#[derive(Debug, Clone)]
pub struct AxNode {
    pub r#ref: El,
    pub role: String,
    pub label: String,
    pub value: Option<String>,
    pub rect: Rect,
    pub secret: bool,
    pub enabled: bool,
    pub ax_role: String,
    pub actions: Vec<String>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub container: Option<String>,
    pub subrole: String,
    pub path: Vec<String>,
    pub index: usize,
}

impl AxNode {
    /// A synthetic node (tests, fakes), as Python's positional constructor.
    pub fn new(
        r: El,
        role: &str,
        label: &str,
        value: Option<&str>,
        rect: Rect,
        secret: bool,
        enabled: bool,
    ) -> AxNode {
        AxNode {
            r#ref: r,
            role: role.into(),
            label: label.into(),
            value: value.map(Into::into),
            rect,
            secret,
            enabled,
            ax_role: String::new(),
            actions: vec![],
            selected: None,
            expanded: None,
            container: None,
            subrole: String::new(),
            path: vec![],
            index: 0,
        }
    }

    /// P19's stable identity (HAND-220): never its value or the snapshot id.
    pub fn identity(&self) -> Map<String, Value> {
        let mut out = Map::new();
        out.insert("path".into(), Value::String(self.path.join("/")));
        out.insert("role".into(), Value::String(self.ax_role.clone()));
        out.insert("subrole".into(), Value::String(self.subrole.clone()));
        out.insert("label".into(), Value::String(self.label.clone()));
        if self.label.is_empty() {
            out.insert("index".into(), Value::from(self.index));
        }
        out
    }
}

/// What a walk read (accessibility.py:92).
#[derive(Debug, Clone, Default)]
pub struct AxTree {
    pub nodes: Vec<AxNode>,
    pub text: String,
    pub title: String,
    pub truncated: bool,
    pub has_web: bool,
    pub truncated_by: String,
    /// A password field somewhere in it; None: couldn't tell (the walk was cut short).
    pub secure: Option<bool>,
    /// The masking inventory; None for a synthetic tree whose node list is its only inventory.
    pub secret_rects: Option<Vec<Rect>>,
    pub web_areas: Vec<Rect>,
    pub empty_web_areas: Vec<Rect>,
    pub headings: Vec<String>,
    pub windows: Vec<Map<String, Value>>,
    pub containers: Vec<String>,
    pub web_empty: bool,
    pub window: Option<El>,
    pub masks_only: bool,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub repeated_nodes: usize,
    /// The walk's own bookkeeping (elements seen, its deadline, its reads); 0.0: no budget (a synthetic tree).
    #[doc(hidden)]
    pub visited: HashMap<u64, Vec<El>>,
    #[doc(hidden)]
    pub deadline: f64,
    #[doc(hidden)]
    pub reads: usize,
}

/// One node's part of a signature.
pub type NodeSig = (
    String,
    String,
    Option<String>,
    Option<bool>,
    Option<bool>,
    bool,
);
/// What a window shows, to tell whether an action did anything.
pub type Signature = (String, String, Vec<String>, Vec<NodeSig>);

impl AxTree {
    /// A synthetic tree (fakes): Python's `AxTree(nodes, text, title)` with `secure=False`.
    pub fn of(nodes: Vec<AxNode>, text: &str, title: &str) -> AxTree {
        AxTree {
            nodes,
            text: text.into(),
            title: title.into(),
            secure: Some(false),
            ..Default::default()
        }
    }

    pub fn signature(&self) -> Signature {
        (
            self.title.clone(),
            self.text.clone(),
            self.containers.clone(),
            self.nodes
                .iter()
                .map(|n| {
                    (
                        n.role.clone(),
                        n.label.clone(),
                        n.value.clone(),
                        n.selected,
                        n.expanded,
                        n.enabled,
                    )
                })
                .collect(),
        )
    }
}

fn eq_rect(a: &Rect, b: &Rect) -> bool {
    a == b
}

/// accessibility.py:636: the web areas no other one holds (each once).
pub fn outermost(areas: &[Rect]) -> Vec<Rect> {
    let mut unique: Vec<Rect> = vec![];
    for a in areas {
        if !unique.iter().any(|u| eq_rect(u, a)) {
            unique.push(*a);
        }
    }
    let holds = |a: &Rect, b: &Rect| {
        a.0 <= b.0 && a.1 <= b.1 && a.0 + a.2 >= b.0 + b.2 && a.1 + a.3 >= b.1 + b.3
    };
    unique
        .iter()
        .filter(|b| !unique.iter().any(|a| a != *b && holds(a, b)))
        .copied()
        .collect()
}

/// Python's `str.strip("‎ ")`.
pub fn strip_marks(s: &str) -> &str {
    s.trim_matches(|c| c == '\u{200e}' || c == ' ')
}

/// The walk and the element operations over one source.
pub struct Ax<S: AxSource> {
    pub src: S,
}

fn point(v: &AxValue) -> (f64, f64) {
    match v {
        AxValue::Point(x, y) | AxValue::Size(x, y) => (*x, *y),
        _ => (0.0, 0.0),
    }
}

impl<S: AxSource> Ax<S> {
    pub fn new(src: S) -> Self {
        Ax { src }
    }

    pub fn attr(&self, el: &El, name: &str) -> AxValue {
        self.src.attr(el, name)
    }
    fn attr_opt(&self, el: Option<&El>, name: &str) -> AxValue {
        el.map(|e| self.src.attr(e, name)).unwrap_or(AxValue::None)
    }
    fn text(v: &AxValue) -> String {
        if v.truthy() {
            v.py_str()
        } else {
            String::new()
        }
    }

    pub fn same(&self, a: Option<&El>, b: Option<&El>) -> bool {
        matches!((a, b), (Some(a), Some(b)) if self.src.same(a, b))
    }
    pub fn role_of(&self, el: &El) -> String {
        Self::text(&self.attr(el, "AXRole"))
    }
    pub fn subrole_of(&self, el: &El) -> String {
        Self::text(&self.attr(el, "AXSubrole"))
    }
    pub fn parent_of(&self, el: &El) -> Option<El> {
        self.attr(el, "AXParent").as_el()
    }

    /// accessibility.py:212: True accepted, False a definite AX error, Err(NoAnswer) when the app didn't answer.
    pub fn perform(&self, el: Option<&El>, action: &str) -> Result<bool, NoAnswer> {
        let Some(el) = el else { return Ok(false) };
        let code = self.src.perform(el, action);
        if code == AX_CANNOT_COMPLETE {
            return Err(NoAnswer::new(action, code));
        }
        Ok(code == 0)
    }

    pub fn password_element(&self, role: &str, el: &El) -> bool {
        PASSWORD_ROLES.contains(&role)
            && PASSWORD_ATTRS
                .iter()
                .any(|a| matches!(self.attr(el, a), AxValue::Str(ref v) if password_metadata(v)))
    }

    /// accessibility.py:236.
    pub fn secure_element(&self, el: Option<&El>) -> bool {
        let Some(el) = el else { return false };
        let role = self.role_of(el);
        role == "AXSecureTextField"
            || self.subrole_of(el) == "AXSecureTextField"
            || self.password_element(&role, el)
    }

    // ---------- windows ----------

    pub fn ax_windows(&self, pid: i32) -> Vec<El> {
        let app = self.src.app(pid);
        let focused = self.attr(&app, "AXFocusedUIElement").as_el();
        let mut candidates: Vec<Option<El>> = vec![
            self.attr(&app, "AXFocusedWindow").as_el(),
            self.attr(&app, "AXMainWindow").as_el(),
            self.attr_opt(focused.as_ref(), "AXWindow").as_el(),
        ];
        candidates.extend(self.attr(&app, "AXWindows").els().into_iter().map(Some));
        let mut out: Vec<El> = vec![];
        for w in candidates.into_iter().flatten() {
            let role = self.role_of(&w);
            if (role == "AXWindow" || role == "AXSheet")
                && !out.iter().any(|x| self.src.same(&w, x))
            {
                out.push(w);
            }
        }
        out
    }

    pub fn app_window(&self, pid: i32) -> Option<El> {
        let wins = self.ax_windows(pid);
        if wins.len() == 1 {
            wins.into_iter().next()
        } else {
            None
        }
    }

    pub fn frame(&self, win: Option<&El>) -> Option<Rect> {
        let win = win?;
        let (x, y) = point(&self.attr(win, "AXPosition"));
        let (w, h) = point(&self.attr(win, "AXSize"));
        if w != 0.0 && h != 0.0 {
            Some((x, y, w, h))
        } else {
            None
        }
    }

    fn frame_of(&self, el: &El) -> Rect {
        let (x, y) = point(&self.attr(el, "AXPosition"));
        let (w, h) = point(&self.attr(el, "AXSize"));
        (x, y, w, h)
    }

    pub fn window_title(&self, win: Option<&El>) -> String {
        match win {
            Some(w) => strip_marks(&Self::text(&self.attr(w, "AXTitle"))).to_string(),
            None => String::new(),
        }
    }

    pub fn is_dialog(&self, win: &El) -> bool {
        DIALOG_SUBROLES.contains(&self.subrole_of(win).as_str())
    }

    pub fn main_window_frame(&self, pid: i32) -> Option<Rect> {
        let app = self.src.app(pid);
        for name in ["AXMainWindow", "AXFocusedWindow"] {
            if let Some(w) = self.attr(&app, name).as_el() {
                if self.role_of(&w) == "AXWindow" {
                    if let Some(f) = self.frame(Some(&w)) {
                        return Some(f);
                    }
                }
            }
        }
        None
    }

    fn near(a: &Rect, b: &Rect, tol: f64) -> bool {
        (a.0 - b.0).abs() <= tol
            && (a.1 - b.1).abs() <= tol
            && (a.2 - b.2).abs() <= tol
            && (a.3 - b.3).abs() <= tol
    }

    pub fn window_minimized(&self, pid: i32, bounds: Rect) -> Option<bool> {
        let matches: Vec<El> = self
            .ax_windows(pid)
            .into_iter()
            .filter(|w| {
                self.frame(Some(w))
                    .is_some_and(|f| Self::near(&f, &bounds, 2.0))
            })
            .collect();
        if matches.len() != 1 {
            return None;
        }
        self.attr(&matches[0], "AXMinimized").bool_or(None)
    }

    /// accessibility.py:311.
    pub fn windows(&self, pid: i32, current: Option<&El>) -> Vec<Map<String, Value>> {
        let reach: Vec<(El, Option<Rect>)> = self
            .ax_windows(pid)
            .into_iter()
            .map(|w| {
                let f = self.frame(Some(&w));
                (w, f)
            })
            .collect();
        let mut out = vec![];
        for cg in self.src.cg_windows(pid) {
            let matches: Vec<&El> = reach
                .iter()
                .filter(|(_, f)| f.is_some_and(|f| Self::near(&f, &cg.bounds, 2.0)))
                .map(|(w, _)| w)
                .collect();
            let ax = if matches.len() == 1 {
                Some(matches[0])
            } else {
                None
            };
            let mut d = Map::new();
            d.insert("id".into(), Value::from(cg.id));
            d.insert("title".into(), Value::String(cg.title.clone()));
            d.insert("onscreen".into(), Value::Bool(cg.onscreen));
            d.insert("reachable".into(), Value::Bool(ax.is_some()));
            d.insert("bounds".into(), rect_json(&cg.bounds));
            d.insert("matching_ax_windows".into(), Value::from(matches.len()));
            if let (Some(ax), Some(cur)) = (ax, current) {
                if self.src.same(ax, cur) {
                    d.insert("current".into(), Value::Bool(true));
                }
            }
            out.push(d);
        }
        out
    }

    /// accessibility.py:328: an exact unique title or an exact observed id.
    pub fn window_named(&self, pid: i32, which: &str) -> Option<El> {
        let want = py_strip(which);
        let reach = self.ax_windows(pid);
        if !want.is_empty() && want.chars().all(is_py_digit) {
            let id: i64 = want.parse().ok()?;
            let cg: Vec<CgWindow> = self
                .src
                .cg_windows(pid)
                .into_iter()
                .filter(|w| w.id == id)
                .collect();
            if cg.len() != 1 {
                return None;
            }
            let mut matches: Vec<El> = reach
                .into_iter()
                .filter(|w| {
                    self.frame(Some(w))
                        .is_some_and(|f| Self::near(&f, &cg[0].bounds, 2.0))
                })
                .collect();
            if matches.len() > 1 && !cg[0].title.is_empty() {
                let t = strip_marks(&cg[0].title).to_string();
                matches.retain(|w| self.window_title(Some(w)) == t);
            }
            return if matches.len() == 1 {
                matches.into_iter().next()
            } else {
                None
            };
        }
        let matches: Vec<El> = reach
            .into_iter()
            .filter(|w| self.window_title(Some(w)) == want)
            .collect();
        if matches.len() == 1 {
            matches.into_iter().next()
        } else {
            None
        }
    }

    /// accessibility.py:354.
    pub fn window_with_frame(
        &self,
        pid: i32,
        bounds: Rect,
        title: Option<&str>,
        current_title: Option<&dyn Fn() -> String>,
    ) -> Option<El> {
        let at = |w: &El| {
            self.frame(Some(w))
                .is_some_and(|f| Self::near(&f, &bounds, 2.0))
        };
        let matches: Vec<El> = self.ax_windows(pid).into_iter().filter(|w| at(w)).collect();
        if matches.len() == 1 {
            return matches.into_iter().next();
        }
        let want = title
            .map(|t| t.trim_matches(|c| c == '\u{200e}' || c == ' '))
            .unwrap_or("")
            .to_string();
        let named: Vec<&El> = matches
            .iter()
            .filter(|w| !want.is_empty() && self.window_title(Some(w)) == want)
            .collect();
        if named.len() != 1 {
            return None;
        }
        let again: Vec<&El> = matches
            .iter()
            .filter(|w| self.window_title(Some(w)) == want)
            .collect();
        if again.len() != 1 || !self.src.same(again[0], named[0]) || !at(named[0]) {
            return None;
        }
        if let Some(now) = current_title {
            if strip_marks(&now()) != want {
                return None;
            }
        }
        Some(named[0].clone())
    }

    pub fn raise_window(&self, pid: i32, bounds: Rect) -> bool {
        let Some(win) = self.window_with_frame(pid, bounds, None, None) else {
            return false;
        };
        self.src.set(&win, "AXMain", AxValue::Bool(true));
        self.src.perform(&win, "AXRaise") == 0
    }

    pub fn element_window(&self, el: &El) -> Option<El> {
        if self.role_of(el) == "AXWindow" {
            Some(el.clone())
        } else {
            self.attr(el, "AXWindow").as_el()
        }
    }

    // ---------- hit tests ----------

    /// accessibility.py:406: (pid, frame of its window) a real click here would reach.
    pub fn hit(&self, x: f64, y: f64) -> Option<(i32, Option<Rect>)> {
        let el = self.src.element_at(&self.src.system(), x, y)?;
        let pid = self.src.pid_of(&el)?;
        let win = if matches!(self.attr(&el, "AXRole"), AxValue::Str(ref r) if r == "AXWindow") {
            Some(el.clone())
        } else {
            self.attr(&el, "AXWindow").as_el()
        };
        Some((pid, self.frame(win.as_ref())))
    }

    pub fn hit_element(&self, pid: i32, x: f64, y: f64) -> Option<El> {
        let el = self.src.element_at(&self.src.app(pid), x, y)?;
        let role = self.role_of(&el);
        if ["AXMenuBar", "AXApplication", "AXMenuBarItem", ""].contains(&role.as_str()) {
            return None;
        }
        Some(el)
    }

    // ---------- the walk ----------

    /// accessibility.py:432.
    pub fn read_tree(
        &self,
        pid: i32,
        browser: bool,
        window: Option<&El>,
        offset: usize,
        budget_s: Option<f64>,
        masks_only: bool,
    ) -> AxTree {
        let walk_s = match budget_s {
            None => WALK_BUDGET_S,
            Some(b) => b.clamp(0.0, WALK_BUDGET_S),
        };
        let started = self.src.monotonic();
        let deadline = started + if browser { walk_s.min(2.0) } else { 0.0 };
        loop {
            let app = self.src.app(pid);
            let win = match window {
                Some(w) => Some(w.clone()),
                None => self.app_window(pid),
            };
            let now = self.src.monotonic();
            let title = match (&win, masks_only) {
                (Some(w), false) => Self::text(&self.attr(w, "AXTitle")),
                _ => String::new(),
            };
            let mut tree = AxTree {
                title,
                window: win.clone(),
                offset,
                masks_only,
                secure: Some(false),
                deadline: if budget_s.is_none() {
                    now + walk_s
                } else {
                    now.max(started + walk_s) + 1e-6
                },
                ..Default::default()
            };
            let mut texts: Vec<String> = vec![];
            let mut seen = 0usize;
            if let Some(w) = &win {
                if masks_only {
                    self.inventory_walk(w, &mut tree, 0);
                } else {
                    let c = if self.is_dialog(w) {
                        Some("dialog".to_string())
                    } else {
                        None
                    };
                    self.walk(w, &mut tree, &mut texts, 0, c, &mut seen, &[], 0);
                }
            }
            for child in self.attr(&app, "AXChildren").els() {
                if let Some(w) = &win {
                    if self.src.same(&child, w) {
                        continue;
                    }
                }
                let role = self.role_of(&child);
                let mut kind = match role.as_str() {
                    "AXMenu" => Some("menu"),
                    "AXPopover" => Some("popover"),
                    "AXSheet" => Some("sheet"),
                    _ => None,
                };
                if kind.is_none() && role == "AXWindow" && self.is_dialog(&child) {
                    kind = Some("dialog");
                }
                let Some(kind) = kind else { continue };
                tree.containers.push(kind.into());
                if masks_only {
                    self.inventory_walk(&child, &mut tree, 1);
                } else {
                    self.walk(
                        &child,
                        &mut tree,
                        &mut texts,
                        1,
                        Some(kind.into()),
                        &mut seen,
                        &[],
                        0,
                    );
                }
            }
            if tree.truncated_by.contains("budget") {
                tree.secure = None;
            }
            tree.text = texts.join(" ");
            tree.truncated = !tree.truncated_by.is_empty();
            tree.windows = if masks_only {
                vec![]
            } else {
                self.windows(pid, win.as_ref())
            };
            if !browser || (tree.has_web && !tree.web_empty) || self.src.monotonic() >= deadline {
                return tree;
            }
            self.src.sleep(0.25);
        }
    }

    fn mark(tree: &mut AxTree, why: &str) {
        if !tree.truncated_by.contains(why) {
            tree.truncated_by = [tree.truncated_by.as_str(), why]
                .iter()
                .filter(|x| !x.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
        }
    }

    fn over_budget(&self, tree: &mut AxTree) -> bool {
        if tree.deadline == 0.0 {
            return false;
        }
        tree.reads += 1;
        if tree.reads > WALK_MAX_NODES || self.src.monotonic() > tree.deadline {
            Self::mark(tree, "budget");
            return true;
        }
        false
    }

    fn repeated(&self, el: &El, tree: &mut AxTree) -> bool {
        let key = self.src.hash(el);
        let bucket = tree.visited.entry(key).or_default();
        if bucket
            .iter()
            .any(|prior| prior.is(el) || self.src.same(prior, el))
        {
            tree.repeated_nodes += 1;
            return true;
        }
        bucket.push(el.clone());
        false
    }

    fn inventory(
        &self,
        el: &El,
        role: &str,
        subrole: &str,
        children: &[El],
        tree: &mut AxTree,
    ) -> bool {
        if tree.secret_rects.is_none() {
            tree.secret_rects = Some(vec![]);
        }
        let secret = role == "AXSecureTextField" || subrole == "AXSecureTextField";
        if secret || self.password_element(role, el) {
            tree.secure = Some(true);
            let f = self.frame_of(el);
            tree.secret_rects.as_mut().expect("set").push(f);
        }
        if role == "AXWebArea" {
            let f = self.frame_of(el);
            tree.web_areas.push(f);
            if children.is_empty() {
                tree.empty_web_areas.push(f);
            }
        }
        secret
    }

    fn inventory_below(&self, children: &[El], tree: &mut AxTree) {
        let mut stack: Vec<El> = children.iter().rev().cloned().collect();
        while let Some(el) = stack.pop() {
            if self.over_budget(tree) {
                return;
            }
            if self.repeated(&el, tree) {
                continue;
            }
            let role = self.role_of(&el);
            let subrole = self.subrole_of(&el);
            let kids = self.attr(&el, "AXChildren").els();
            self.inventory(&el, &role, &subrole, &kids, tree);
            stack.extend(kids.into_iter().rev());
        }
    }

    fn inventory_walk(&self, root: &El, tree: &mut AxTree, depth: usize) {
        let mut stack: Vec<(El, usize)> = vec![(root.clone(), depth)];
        while let Some((el, d)) = stack.pop() {
            if self.over_budget(tree) {
                return;
            }
            if self.repeated(&el, tree) {
                continue;
            }
            let role = self.role_of(&el);
            let subrole = self.subrole_of(&el);
            let kids = self.attr(&el, "AXChildren").els();
            self.inventory(&el, &role, &subrole, &kids, tree);
            if role == "AXWebArea" && d <= MAX_DEPTH + 1 {
                tree.has_web = true;
                tree.web_empty = tree.web_empty || kids.is_empty();
            }
            stack.extend(kids.into_iter().rev().map(|k| (k, d + 1)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &self,
        el: &El,
        tree: &mut AxTree,
        texts: &mut Vec<String>,
        depth: usize,
        container: Option<String>,
        seen: &mut usize,
        path: &[String],
        index: usize,
    ) {
        if self.over_budget(tree) || self.repeated(el, tree) {
            return;
        }
        let role = self.role_of(el);
        let subrole = self.subrole_of(el);
        let children = self.attr(el, "AXChildren").els();
        let secret = self.inventory(el, &role, &subrole, &children, tree);
        if role == "AXWebArea" {
            tree.has_web = true;
            tree.web_empty = tree.web_empty || children.is_empty();
        }
        let mut container = container;
        if role == "AXSheet" && container.is_none() {
            container = Some("sheet".into());
            tree.containers.push("sheet".into());
        }
        if depth > MAX_DEPTH {
            Self::mark(tree, "depth");
            self.inventory_below(&children, tree);
            return;
        }
        let order = *seen;
        *seen += 1;
        let capped = tree.nodes.len() >= MAX_ELEMENTS;
        if order >= tree.offset && !capped {
            if TEXT_ROLES.contains(&role.as_str()) {
                let v = self.attr(el, "AXValue").or(self.attr(el, "AXTitle"));
                if v.truthy() {
                    texts.push(v.py_str());
                    if role == "AXHeading" {
                        tree.headings.push(v.py_str());
                    }
                }
            }
            let acts = self.src.actions(el);
            if !role.is_empty() || !subrole.is_empty() || !acts.is_empty() {
                let node = self.node(
                    el,
                    &role,
                    &subrole,
                    secret,
                    &acts,
                    container.clone(),
                    &children,
                    path,
                    index,
                );
                tree.nodes.push(node);
            }
        } else if order >= tree.offset && capped {
            Self::mark(tree, "elements");
            tree.next_offset = Some(tree.offset + tree.nodes.len());
        }
        let mut below: Vec<String> = path.to_vec();
        below.push(role.clone());
        for (i, child) in children.iter().enumerate() {
            self.walk(
                child,
                tree,
                texts,
                depth + 1,
                container.clone(),
                seen,
                &below,
                i,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &self,
        el: &El,
        role: &str,
        subrole: &str,
        secret: bool,
        acts: &[String],
        container: Option<String>,
        children: &[El],
        path: &[String],
        index: usize,
    ) -> AxNode {
        let kind = if subrole == "AXSearchField" {
            "searchbox".to_string()
        } else if let Some(k) = role_kind(role) {
            k.to_string()
        } else if let Some(rest) = role.strip_prefix("AX") {
            ferb_py::text::lower(rest)
        } else if role.is_empty() {
            "element".into()
        } else {
            role.to_string()
        };
        let mut label = Self::text(
            &self
                .attr(el, "AXTitle")
                .or(self.attr(el, "AXDescription"))
                .or(self.attr(el, "AXPlaceholderValue"))
                .or(self.attr(el, "AXHelp")),
        );
        if label.is_empty() && role_kind(role).is_none() {
            let parts: Vec<String> = children
                .iter()
                .filter(|c| TEXT_ROLES.contains(&self.role_of(c).as_str()))
                .map(|c| Self::text(&self.attr(c, "AXValue").or(self.attr(c, "AXTitle"))))
                .collect();
            label = py_strip(&parts.join(" ")).to_string();
        }
        let value = if secret {
            Some(if self.attr(el, "AXNumberOfCharacters").truthy() {
                "filled".to_string()
            } else {
                String::new()
            })
        } else if VALUE_ROLES.contains(&role) {
            Some(Self::text(&self.attr(el, "AXValue")))
        } else {
            match self.attr(el, "AXValue") {
                AxValue::Str(s) if !s.is_empty() => Some(s),
                _ => None,
            }
        };
        let (x, y) = point(&self.attr(el, "AXPosition"));
        let (w, h) = point(&self.attr(el, "AXSize"));
        let enabled = self.attr(el, "AXEnabled");
        AxNode {
            r#ref: el.clone(),
            role: kind,
            label,
            value,
            rect: (x, y, w, h),
            secret,
            enabled: enabled.bool_or(Some(true)).unwrap_or(true),
            ax_role: role.to_string(),
            actions: acts
                .iter()
                .filter_map(|a| action_name(a).map(String::from))
                .collect(),
            selected: self.attr(el, "AXSelected").bool_or(None),
            expanded: self.attr(el, "AXExpanded").bool_or(None),
            container,
            subrole: subrole.to_string(),
            path: path.to_vec(),
            index,
        }
    }

    /// accessibility.py:617: a browser window's page areas, without walking into them; None past the budget.
    pub fn web_areas_of(&self, win: &El) -> Option<Vec<Rect>> {
        let deadline = self.src.monotonic() + WEB_AREAS_BUDGET_S;
        let mut found = vec![];
        let mut stack = vec![win.clone()];
        let mut read = 0;
        while let Some(el) = stack.pop() {
            read += 1;
            if read > WEB_AREAS_MAX || self.src.monotonic() > deadline {
                return None;
            }
            if self.role_of(&el) == "AXWebArea" {
                found.push(self.frame_of(&el));
            } else {
                stack.extend(self.attr(&el, "AXChildren").els());
            }
        }
        Some(found)
    }

    pub fn value(&self, el: &El) -> Option<String> {
        let v = self.attr(el, "AXValue");
        if v.is_none() {
            None
        } else {
            Some(v.py_str())
        }
    }

    /// AXManualAccessibility for Electron apps (accessibility.py:684).
    pub fn expose_tree(&self, pid: i32) -> bool {
        self.src.set(
            &self.src.app(pid),
            "AXManualAccessibility",
            AxValue::Bool(true),
        ) == 0
    }

    fn title_of(&self, el: &El) -> String {
        py_strip(strip_marks(&Self::text(&self.attr(el, "AXTitle")))).to_string()
    }

    // ---------- menus ----------

    pub fn menus(&self, pid: i32) -> Vec<Value> {
        let bar = self.attr(&self.src.app(pid), "AXMenuBar").as_el();
        let tops = self.attr_opt(bar.as_ref(), "AXChildren").els();
        let mut out = vec![];
        for top in tops.iter().skip(1) {
            let mut items: Vec<String> = vec![];
            for menu in self.attr(top, "AXChildren").els() {
                for item in self.attr(&menu, "AXChildren").els() {
                    let t = self.title_of(&item);
                    if t.is_empty() {
                        continue;
                    }
                    let subs: Vec<String> = self
                        .attr(&item, "AXChildren")
                        .els()
                        .iter()
                        .flat_map(|m| self.attr(m, "AXChildren").els())
                        .map(|x| self.title_of(&x))
                        .collect();
                    if subs.iter().any(|s| !s.is_empty()) {
                        items.extend(
                            subs.iter()
                                .filter(|s| !s.is_empty())
                                .map(|s| format!("{t} > {s}")),
                        );
                    } else {
                        items.push(t);
                    }
                }
            }
            let mut m = Map::new();
            m.insert("menu".into(), Value::String(self.title_of(top)));
            m.insert("items".into(), Value::from(items));
            out.push(Value::Object(m));
        }
        out
    }

    fn menu_item_at(&self, pid: i32, path: &[String]) -> Option<El> {
        let mut level = self.attr(&self.src.app(pid), "AXMenuBar").as_el();
        let mut el = None;
        for (i, name) in path.iter().enumerate() {
            let mut kids = self.attr_opt(level.as_ref(), "AXChildren").els();
            if i > 0 {
                kids = kids
                    .iter()
                    .flat_map(|m| self.attr(m, "AXChildren").els())
                    .collect();
            }
            let want = py_strip(name);
            let matches: Vec<El> = kids
                .into_iter()
                .filter(|k| self.title_of(k) == want)
                .collect();
            el = if matches.len() == 1 {
                matches.into_iter().next()
            } else {
                None
            };
            el.as_ref()?;
            level = el.clone();
        }
        el
    }

    pub fn press_menu(&self, pid: i32, path: &[String]) -> Result<bool, NoAnswer> {
        match self.menu_item_at(pid, path) {
            Some(el) => self.perform(Some(&el), "AXPress"),
            None => Ok(false),
        }
    }

    /// accessibility.py:733: (item, "Edit > Select All", enabled) for a key equivalent; None unless unique.
    pub fn menu_item_for_shortcut(
        &self,
        pid: i32,
        key: &str,
        modifiers: &[String],
    ) -> Option<(El, String, bool)> {
        let mut want: i64 = modifiers
            .iter()
            .map(|m| match m.as_str() {
                "shift" => 1,
                "alt" => 2,
                "ctrl" => 4,
                _ => 0,
            })
            .sum();
        if !modifiers.iter().any(|m| m == "cmd") {
            want |= 8;
        }
        let bar = self.attr(&self.src.app(pid), "AXMenuBar").as_el();
        let mut stack: Vec<(El, Vec<String>)> = vec![];
        for top in self.attr_opt(bar.as_ref(), "AXChildren").els() {
            for m in self.attr(&top, "AXChildren").els() {
                stack.push((m, vec![self.title_of(&top)]));
            }
        }
        let mut matches = vec![];
        let lower_key = ferb_py::text::lower(key);
        while let Some((menu, path)) = stack.pop() {
            for item in self.attr(&menu, "AXChildren").els() {
                let t = self.title_of(&item);
                let ch = Self::text(&self.attr(&item, "AXMenuItemCmdChar"));
                let mods = self.attr(&item, "AXMenuItemCmdModifiers");
                if !ch.is_empty() && ferb_py::text::lower(&ch) == lower_key {
                    if let Some(m) = int_of(&mods) {
                        if m == want {
                            let enabled = self
                                .attr(&item, "AXEnabled")
                                .bool_or(Some(true))
                                .unwrap_or(true);
                            let mut p = path.clone();
                            p.push(t.clone());
                            matches.push((item.clone(), p.join(" > "), enabled));
                        }
                    }
                }
                let mut p = path.clone();
                p.push(t.clone());
                for sub in self.attr(&item, "AXChildren").els() {
                    stack.push((sub, p.clone()));
                }
            }
        }
        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }

    pub fn open_menus(&self, pid: i32) -> Vec<El> {
        self.attr(&self.src.app(pid), "AXChildren")
            .els()
            .into_iter()
            .filter(|c| self.role_of(c) == "AXMenu")
            .collect()
    }

    pub fn popup_menu(&self, el: &El, pid: i32) -> Option<El> {
        for c in self.attr(el, "AXChildren").els() {
            if self.role_of(&c) == "AXMenu" {
                return Some(c);
            }
        }
        let menus = self.open_menus(pid);
        if menus.len() == 1 {
            menus.into_iter().next()
        } else {
            None
        }
    }

    pub fn menu_items(&self, menu: &El) -> Vec<(El, String)> {
        self.attr(menu, "AXChildren")
            .els()
            .into_iter()
            .filter(|i| self.role_of(i) == "AXMenuItem")
            .map(|i| {
                let t = self.title_of(&i);
                (i, t)
            })
            .collect()
    }

    pub fn menu_item_shortcut(&self, pid: i32, path: &[String]) -> Option<(String, i64)> {
        let el = self.menu_item_at(pid, path)?;
        self.item_shortcut(&el)
    }

    pub fn item_shortcut(&self, item: &El) -> Option<(String, i64)> {
        let ch = self.attr(item, "AXMenuItemCmdChar");
        let mods = self.attr(item, "AXMenuItemCmdModifiers");
        if ch.truthy() {
            int_of(&mods).map(|m| (ch.py_str(), m))
        } else {
            None
        }
    }

    // ---------- the terminal facts (D31) ----------

    fn facts_of(&self, el: &El) -> Map<String, Value> {
        let mut facts = Map::new();
        for (key, name) in SURFACE_ATTRS {
            let v = self.attr(el, name);
            if key == "dom_classes" {
                let list = strings(&v);
                if !list.is_empty() {
                    facts.insert(key.into(), Value::from(list));
                }
            } else if !(v.is_none() || matches!(v, AxValue::Str(ref s) if s.is_empty())) {
                facts.insert(key.into(), Value::String(v.py_str()));
            }
        }
        facts
    }

    pub fn surface_facts(&self, el: Option<&El>) -> Vec<Map<String, Value>> {
        let mut chain = vec![];
        let mut cur = el.cloned();
        while let Some(e) = cur {
            if chain.len() >= SURFACE_DEPTH {
                break;
            }
            let facts = self.facts_of(&e);
            let app = facts.get("role").and_then(Value::as_str) == Some("AXApplication");
            chain.push(facts);
            if app {
                break;
            }
            cur = self.attr(&e, "AXParent").as_el();
        }
        chain
    }

    /// accessibility.py:884: each element's own facts in the window holding `el`, at most WINDOW_WALK_MAX; a None
    /// last when cut short.
    pub fn window_facts(&self, el: &El) -> Vec<Option<Map<String, Value>>> {
        let mut out = vec![];
        let mut stack: Vec<El> = self.attr(el, "AXWindow").as_el().into_iter().collect();
        let mut read = 0;
        while let Some(node) = stack.pop() {
            if read >= WINDOW_WALK_MAX {
                out.push(None);
                return out;
            }
            read += 1;
            out.push(Some(self.facts_of(&node)));
            stack.extend(self.attr(&node, "AXChildren").els().into_iter().rev());
        }
        out
    }

    /// accessibility.py:814; `browser_page` is policy/confirm.py's check.
    pub fn page_url(&self, el: &El, browser_page: &dyn Fn(Option<&str>) -> bool) -> Option<String> {
        let mut found: Option<String> = None;
        let mut depth = 0;
        let mut cur = Some(el.clone());
        while let Some(e) = cur {
            if depth >= SURFACE_DEPTH {
                break;
            }
            let role = self.attr(&e, "AXRole");
            let role = role.as_str().unwrap_or("");
            if role == "AXWebArea" {
                let raw = self.attr(&e, "AXURL");
                let text = match &raw {
                    AxValue::Url { absolute, .. } => absolute.clone(),
                    v if v.truthy() => v.py_str(),
                    _ => String::new(),
                };
                if browser_page(Some(&text)) {
                    return Some(text);
                }
                if !text.is_empty() {
                    found = Some(text);
                }
            }
            if role == "AXWindow" || role == "AXApplication" {
                break;
            }
            cur = self.attr(&e, "AXParent").as_el();
            depth += 1;
        }
        found
    }

    pub fn file_of(&self, el: &El) -> Option<String> {
        for candidate in [Some(el.clone()), self.attr(el, "AXParent").as_el()]
            .into_iter()
            .flatten()
        {
            if let AxValue::Url { path: Some(p), .. } = self.attr(&candidate, "AXURL") {
                if !p.is_empty() {
                    return Some(p);
                }
            }
        }
        None
    }

    pub fn selected_items(&self, pid: i32) -> Vec<El> {
        match self.attr(&self.src.app(pid), "AXFocusedUIElement").as_el() {
            Some(f) => self.attr(&f, "AXSelectedChildren").els(),
            None => vec![],
        }
    }

    /// accessibility.py:863: Monaco's input inside the editor group VS Code marks active.
    pub fn active_editor(&self, pid: i32) -> Option<El> {
        let app = self.src.app(pid);
        let win = self
            .attr(&app, "AXFocusedWindow")
            .or(self.attr(&app, "AXMainWindow"))
            .as_el();
        let mut stack: Vec<(El, bool)> = win.into_iter().map(|w| (w, false)).collect();
        let mut read = 0;
        while let Some((el, inside)) = stack.pop() {
            if read >= ACTIVE_EDITOR_MAX {
                break;
            }
            read += 1;
            let classes: Vec<String> = strings(&self.attr(&el, "AXDOMClassList"))
                .iter()
                .map(|c| ferb_sanitize::parity::casefold(c))
                .collect();
            let inside = inside
                || ["editor-group-container", "active"]
                    .iter()
                    .all(|c| classes.iter().any(|x| x == c));
            if inside
                && classes
                    .iter()
                    .any(|c| c == "inputarea" || c == "native-edit-context")
            {
                return Some(el);
            }
            stack.extend(
                self.attr(&el, "AXChildren")
                    .els()
                    .into_iter()
                    .rev()
                    .map(|k| (k, inside)),
            );
        }
        None
    }

    pub fn selected_text(&self, el: &El) -> Option<String> {
        match self.attr(el, "AXSelectedText") {
            AxValue::Str(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    pub fn document_of(&self, el: &El) -> Option<String> {
        let win = self.attr(el, "AXWindow").as_el();
        let doc = self.attr_opt(win.as_ref(), "AXDocument");
        let AxValue::Str(doc) = doc else { return None };
        if doc.is_empty() {
            return None;
        }
        match doc.strip_prefix("file:") {
            Some(rest) => Some(unquote(url_path(rest))),
            None => Some(doc),
        }
    }

    pub fn place_of(&self, el: &El) -> (String, String) {
        let window = self.attr(el, "AXWindow").as_el();
        let label = self.attr(el, "AXTitle").or(self.attr(el, "AXDescription"));
        (
            self.window_title(window.as_ref()),
            if label.truthy() {
                label.py_str()
            } else {
                String::new()
            },
        )
    }

    pub fn focused_element(&self, pid: i32) -> Option<El> {
        self.attr(&self.src.app(pid), "AXFocusedUIElement").as_el()
    }

    pub fn has_focus(&self, pid: i32, el: &El) -> Option<bool> {
        let mut focused = self.focused_element(pid)?;
        for _ in 0..30 {
            if self.src.same(&focused, el) {
                return Some(true);
            }
            match self.attr(&focused, "AXParent").as_el() {
                Some(p) => focused = p,
                None => break,
            }
        }
        Some(false)
    }

    pub fn set_value(&self, el: &El, text: &str) -> Result<bool, NoAnswer> {
        let code = self.src.set(el, "AXValue", AxValue::Str(text.into()));
        if code == AX_CANNOT_COMPLETE {
            return Err(NoAnswer::new("AXValue", code));
        }
        Ok(code == 0)
    }

    pub fn selected_range(&self, el: &El) -> Option<(i64, i64)> {
        match self.attr(el, "AXSelectedTextRange") {
            AxValue::Range(a, b) => Some((a, b)),
            _ => None,
        }
    }

    pub fn set_selected_range(&self, el: &El, start: i64, length: i64) -> bool {
        self.src
            .set(el, "AXSelectedTextRange", AxValue::Range(start, length))
            == 0
    }

    pub fn set_selected_text(&self, el: &El, text: &str) -> bool {
        self.src
            .set(el, "AXSelectedText", AxValue::Str(text.into()))
            == 0
    }

    pub fn focus(&self, el: &El) -> bool {
        self.src.set(el, "AXFocused", AxValue::Bool(true)) == 0
    }

    // ---------- scrolling ----------

    fn scroll_container(&self, el: &El) -> bool {
        ["AXScrollArea", "AXList", "AXTable", "AXOutline"].contains(&self.role_of(el).as_str())
            || ["AXVerticalScrollBar", "AXHorizontalScrollBar"]
                .iter()
                .any(|n| !self.attr(el, n).is_none())
    }

    pub fn scroll_area_of(&self, el: &El) -> Option<El> {
        let mut node = Some(el.clone());
        for _ in 0..16 {
            let n = node?;
            if self.scroll_container(&n) {
                return Some(n);
            }
            node = self.attr(&n, "AXParent").as_el();
        }
        None
    }

    pub fn scroll_area_in(&self, root: &El) -> Option<El> {
        let mut queue = std::collections::VecDeque::from(vec![root.clone()]);
        let mut seen = 0;
        let mut found: Vec<El> = vec![];
        while seen < 2000 {
            let Some(el) = queue.pop_front() else { break };
            seen += 1;
            if self.scroll_container(&el) {
                found.push(el);
                if found.len() > 1 {
                    return None;
                }
                continue;
            }
            queue.extend(self.attr(&el, "AXChildren").els());
        }
        if found.len() == 1 && queue.is_empty() {
            found.pop()
        } else {
            None
        }
    }

    pub fn scroll_content(&self, area: &El) -> Option<El> {
        let children: Vec<El> = self
            .attr(area, "AXChildren")
            .els()
            .into_iter()
            .filter(|c| self.role_of(c) != "AXScrollBar")
            .collect();
        if children.len() == 1 {
            children.into_iter().next()
        } else {
            None
        }
    }

    pub fn scroll_bar(&self, area: &El, vertical: bool) -> Option<El> {
        self.attr(
            area,
            if vertical {
                "AXVerticalScrollBar"
            } else {
                "AXHorizontalScrollBar"
            },
        )
        .as_el()
    }

    fn visible_scroll_items(&self, area: &El) -> Vec<(String, Rect, String)> {
        let Some((x, y, w, h)) = self.frame(Some(area)) else {
            return vec![];
        };
        let rows = self
            .attr(area, "AXVisibleRows")
            .or(self.attr(area, "AXChildren"))
            .els();
        let mut queue: std::collections::VecDeque<(El, usize)> =
            rows.into_iter().take(100).map(|r| (r, 0)).collect();
        let mut found = vec![];
        let mut seen = 0;
        while seen < 100 {
            let Some((row, depth)) = queue.pop_front() else {
                break;
            };
            seen += 1;
            let rect = self.frame(Some(&row));
            let role = self.role_of(&row);
            if role == "AXScrollBar" {
                continue;
            }
            if let Some((rx, ry, rw, rh)) = rect {
                if rw <= 0.0
                    || rh <= 0.0
                    || rx + rw <= x
                    || ry + rh <= y
                    || rx >= x + w
                    || ry >= y + h
                {
                    continue;
                }
                let identity: Vec<String> = ["AXIdentifier", "AXTitle", "AXDescription"]
                    .iter()
                    .map(|k| Self::text(&self.attr(&row, k)))
                    .collect();
                // An opaque digest of the row's identity, compared only with itself before and after a step (Python
                // uses blake2b of its repr; the bytes never leave this function).
                let digest = {
                    use sha2::Digest;
                    hex::encode(&sha2::Sha256::digest(identity.join("\u{0}").as_bytes())[..12])
                };
                found.push((role, (rx, ry, rw, rh), digest));
            }
            if depth < 2 {
                queue.extend(
                    self.attr(&row, "AXChildren")
                        .els()
                        .into_iter()
                        .take(100)
                        .map(|c| (c, depth + 1)),
                );
            }
        }
        found
    }

    /// accessibility.py:1104.
    pub fn scroll_position(&self, area: Option<&El>, content: Option<&El>) -> ScrollPosition {
        let Some(area) = area else {
            return ScrollPosition::default();
        };
        let v = self.attr_opt(self.scroll_bar(area, true).as_ref(), "AXValue");
        let h = self.attr_opt(self.scroll_bar(area, false).as_ref(), "AXValue");
        ScrollPosition {
            vertical: fraction(&v),
            horizontal: fraction(&h),
            visible: content.and_then(|c| match self.attr(c, "AXVisibleCharacterRange") {
                AxValue::Range(a, b) => Some((a, b)),
                _ => None,
            }),
            items: Some(self.visible_scroll_items(area)),
        }
    }
}

/// Where a scroll pane is: scroll bar fractions, the visible text range and a fingerprint of what it shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScrollPosition {
    pub vertical: Option<f64>,
    pub horizontal: Option<f64>,
    pub visible: Option<(i64, i64)>,
    /// None: no position read at all (Python's empty tuple).
    pub items: Option<Vec<(String, Rect, String)>>,
}

impl ScrollPosition {
    pub fn is_empty(&self) -> bool {
        self.items.is_none()
            && self.vertical.is_none()
            && self.horizontal.is_none()
            && self.visible.is_none()
    }
}

fn fraction(v: &AxValue) -> Option<f64> {
    let f = match v {
        AxValue::Int(n) => *n as f64,
        AxValue::Float(f) => *f,
        AxValue::Str(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if (0.0..=1.0).contains(&f) {
        Some(f)
    } else {
        None
    }
}

fn int_of(v: &AxValue) -> Option<i64> {
    match v {
        AxValue::Int(n) => Some(*n),
        AxValue::Bool(b) => Some(*b as i64),
        AxValue::Float(f) => Some(f.trunc() as i64),
        AxValue::Str(s) => s.trim().parse().ok(),
        _ => None,
    }
}

const SURFACE_ATTRS: [(&str, &str); 8] = [
    ("role", "AXRole"),
    ("subrole", "AXSubrole"),
    ("role_description", "AXRoleDescription"),
    ("identifier", "AXIdentifier"),
    ("dom_identifier", "AXDOMIdentifier"),
    ("dom_classes", "AXDOMClassList"),
    ("title", "AXTitle"),
    ("description", "AXDescription"),
];

fn strings(v: &AxValue) -> Vec<String> {
    match v {
        AxValue::None => vec![],
        AxValue::Str(s) => vec![s.clone()],
        AxValue::List(items) => items.iter().map(|i| i.py_str()).collect(),
        _ => vec![],
    }
}

pub fn rect_json(r: &Rect) -> Value {
    Value::from(vec![r.0, r.1, r.2, r.3])
}

fn is_py_digit(c: char) -> bool {
    ferb_py::text::is_digit(c)
}

fn py_strip(s: &str) -> &str {
    ferb_py::value::strip(s)
}

/// `urlsplit(doc).path` for "file:" + rest.
fn url_path(rest: &str) -> &str {
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    if let Some(after) = rest.strip_prefix("//") {
        match after.find('/') {
            Some(i) => &after[i..],
            None => "",
        }
    } else {
        rest
    }
}

/// `urllib.parse.unquote`: %XX sequences as UTF-8 bytes, invalid sequences kept, undecodable bytes replaced.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = h {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An AX action the app didn't answer (kAXErrorCannotComplete): it may have been delivered, so neither done nor
/// failed: the gateway records the outcome as unknown (P19).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{action}: the app didn't answer in time (AX error {code}); the action may have happened")]
pub struct NoAnswer {
    pub action: String,
    pub code: i32,
}

impl NoAnswer {
    pub fn new(action: &str, code: i32) -> Self {
        NoAnswer {
            action: action.into(),
            code,
        }
    }
}

pub fn center(n: &AxNode) -> (f64, f64) {
    let (x, y, w, h) = n.rect;
    (x + w / 2.0, y + h / 2.0)
}
