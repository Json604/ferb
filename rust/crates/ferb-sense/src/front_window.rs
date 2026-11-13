//! The owner's front window at the moment they send a message (`ferb/sense/front_window.py`, L12). It never brings
//! anything forward (rule 10), never chooses a window by what it shows and never keeps an unredacted pixel.
//!
//! The front window is the topmost on-screen normal window whose owner process isn't FERB's own; of that window's app,
//! its main window when Accessibility names one on screen. The window of the app hosting the sending CLI is skipped
//! while it is in front. Kept: app, bundle id, window title, the active tab of that very browser window, and one
//! screenshot of that window only (redacted twice: secret fields masked in the capture, then on-device redaction;
//! withheld when either fails). An app on the exclusion list is never looked at or named; a site on it is seen but
//! its origin and URL never name the host.

use std::sync::Mutex;

use ferb_core::interfaces::BoxFuture;
use ferb_sanitize::{origin, EXCLUDED};
use serde_json::Value;

use crate::observation::{dict, text_or_empty, Dict, Redact};

/// front_window.py:47: FERB's own apps and the code identifiers of its non-bundle programs.
pub const FERB_APPS: [&str; 6] = [
    "com.ferb.ferb",
    "dev.ferb.notch",
    "com.ferb.app",
    "com.ferb.daemon",
    "com.ferb.python",
    "com.ferb.secrets",
];

/// front_window.py:51.
pub fn excluded_app_text() -> String {
    format!("an app FERB doesn't look at ({EXCLUDED}) is in front: no title, page or screenshot")
}

/// act/mac/screenshot.py:30 `Window`: an on-screen window as the window server lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: i64,
    pub pid: i64,
    /// x, y, w, h in screen points (top-left origin)
    pub bounds: (f64, f64, f64, f64),
    pub title: String,
    pub layer: i64,
    pub alpha: f64,
}

/// front_window.py:54.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrontWindow {
    pub app: String,
    pub bundle_id: String,
    pub title: String,
    pub url: Option<String>,
    pub tab_title: Option<String>,
    /// the evidence-store row of the redacted screenshot; None: withheld
    pub image_ref: Option<String>,
    pub origin: String,
    pub withheld: Option<String>,
    /// those same redacted bytes, in memory
    pub image: Option<Vec<u8>>,
    pub tab_note: Option<String>,
    pub tab_source: Option<String>,
    pub masking: String,
}

impl FrontWindow {
    /// front_window.py:69: the labelled facts the planner is given.
    pub fn facts(&self) -> Dict {
        let mut out = dict([("app", Value::from(self.app.clone()))]);
        if !self.bundle_id.is_empty() {
            out.insert("bundle_id".into(), Value::from(self.bundle_id.clone()));
        }
        if !self.title.is_empty() {
            out.insert("window_title".into(), Value::from(self.title.clone()));
        }
        let opt = |v: &Option<String>| v.clone().map_or(Value::Null, Value::from);
        let truthy = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        if truthy(&self.url) || truthy(&self.tab_title) {
            out.insert("url".into(), opt(&self.url));
            out.insert("tab_title".into(), opt(&self.tab_title));
            if truthy(&self.tab_source) {
                out.insert("tab_source".into(), opt(&self.tab_source));
            }
        } else if truthy(&self.tab_note) {
            out.insert("url".into(), Value::Null);
            out.insert("tab_title".into(), Value::Null);
            out.insert("tab_note".into(), opt(&self.tab_note));
        }
        out
    }
}

/// Why no window of the browser could be told to be the captured one (Python's `LookupError`; `ambiguous`:
/// `browser_tabs.Ambiguous`, several could be).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabLookup {
    pub message: String,
    pub ambiguous: bool,
}

/// Why a screenshot failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShotError {
    /// FERB's own words (`Refused`): why this window gave no picture.
    Refused(String),
    /// Anything else, by its kind (Python's exception type name).
    Failed(String),
}

/// A screenshot: the JPEG, how its secret fields were found ("" when not worth saying) and, for a browser window,
/// the tab the paired extension named for this very window.
pub type Shot = (Vec<u8>, String, Option<(String, String)>);

/// front_window.py:86 `Sources`: what a capture reads. The daemon supplies the real ones; tests supply fakes.
pub trait Sources: Send + Sync {
    /// On-screen normal windows, front to back.
    fn windows(&self) -> Vec<Window>;
    /// A process's (app name, bundle id); None: not an app.
    fn app_of(&self, pid: i64) -> Option<(String, String)>;
    /// (pid, bundle id): a FERB process ([`ferb_process`]).
    fn own(&self, pid: i64, bundle: &str) -> bool;
    /// (bundle id, app name, url): on the owner's exclusion list.
    fn excluded(&self, bundle: &str, name: &str, url: Option<&str>) -> bool;
    /// That browser window's active tab (url, title); None: not read now.
    fn tab(
        &self,
        bundle: &str,
        w: &Window,
        after_shot: bool,
    ) -> Result<Option<(String, String)>, TabLookup>;
    fn shoot<'a>(&'a self, w: &'a Window) -> BoxFuture<'a, Result<Shot, ShotError>>;
    /// On-device image redaction; `Err` when it can't redact.
    fn redact_image(&self, raw: &[u8]) -> Result<Vec<u8>, String>;
    fn redact(&self) -> &dyn Redact;
    /// (image, facts, origin) to an evidence id; `Err(kind)` when it can't be kept.
    fn keep<'a>(
        &'a self,
        image: &'a [u8],
        facts: &'a Dict,
        origin: &'a str,
    ) -> BoxFuture<'a, Result<String, String>>;
    /// The process the message came from.
    fn client(&self) -> Option<i64>;
    /// A process's parent.
    fn parent(&self, pid: i64) -> Option<i64>;
    /// A process's main window frame (x, y, w, h) from Accessibility; None: it can't say.
    fn main_frame(&self, pid: i64) -> Option<(f64, f64, f64, f64)>;
    /// `act/mac/browsers.py` `is_browser`.
    fn is_browser(&self, bundle: &str) -> bool;
    /// `act/mac/tools.py` `_sources`: the read sources of an app's window.
    fn read_sources(&self, bundle: &str) -> Vec<(String, String)>;
}

/// front_window.py:128 `ferb_process`: one of FERB's own apps, the daemon, or a process the daemon started that has
/// no app bundle of its own (walked up through the parents).
pub fn ferb_process(
    pid: i64,
    bundle_id: &str,
    daemon_pid: i64,
    parent: &dyn Fn(i64) -> Option<i64>,
) -> bool {
    if FERB_APPS.contains(&ferb_py::text::lower(bundle_id).as_str()) || pid == daemon_pid {
        return true;
    }
    if !bundle_id.is_empty() {
        return false;
    }
    let mut seen = std::collections::HashSet::new();
    let mut pid = pid;
    while pid > 1 && !seen.contains(&pid) {
        if pid == daemon_pid {
            return true;
        }
        seen.insert(pid);
        pid = parent(pid).unwrap_or(0);
    }
    false
}

/// front_window.py:146 `sending_app`: the app hosting a client process (itself included): the first ancestor with a
/// bundle id. None: no app above it.
pub fn sending_app(
    pid: i64,
    app_of: &dyn Fn(i64) -> Option<(String, String)>,
    parent: &dyn Fn(i64) -> Option<i64>,
) -> Option<i64> {
    let mut seen = std::collections::HashSet::new();
    let mut pid = pid;
    while pid > 1 && !seen.contains(&pid) {
        if app_of(pid).is_some_and(|(_, b)| !b.is_empty()) {
            return Some(pid);
        }
        seen.insert(pid);
        pid = parent(pid).unwrap_or(0);
    }
    None
}

/// front_window.py:168 `_topmost`.
pub fn topmost(src: &dyn Sources) -> Option<(Window, String, String)> {
    let windows = src.windows();
    let sender = src
        .client()
        .filter(|c| *c != 0)
        .and_then(|c| sending_app(c, &|p| src.app_of(p), &|p| src.parent(p)));
    let mut sender_skipped = false;
    for w in &windows {
        let (name, bundle) = src.app_of(w.pid).unwrap_or_default();
        if src.own(w.pid, &bundle) {
            continue;
        }
        if Some(w.pid) == sender && !sender_skipped {
            sender_skipped = true;
            continue;
        }
        let mut chosen = w.clone();
        if let Some(main) = src.main_frame(w.pid) {
            let m = [main.0, main.1, main.2, main.3];
            if let Some(x) = windows.iter().find(|x| {
                let b = [x.bounds.0, x.bounds.1, x.bounds.2, x.bounds.3];
                x.pid == w.pid && b.iter().zip(m).all(|(a, b)| (a - b).abs() <= 2.0)
            }) {
                chosen = x.clone();
            }
        }
        return Some((chosen, name, bundle));
    }
    None
}

type Tab = (
    Option<(Option<String>, Option<String>)>,
    Option<String>,
    bool,
);

fn read_tab(src: &dyn Sources, w: &Window, bundle: &str, after: bool) -> Tab {
    match src.tab(bundle, w, after) {
        Err(e) => (None, Some(e.message), e.ambiguous),
        Ok(t) => (
            t.map(|(u, ti)| ((!u.is_empty()).then_some(u), (!ti.is_empty()).then_some(ti))),
            None,
            false,
        ),
    }
}

/// front_window.py:202 `capture`: the owner's front window now, or None when no window but FERB's is in front.
/// `stage` names each step as it begins (a capture stopped by the planner's wait names the one it was on).
pub async fn capture(src: &dyn Sources, stage: &Mutex<Vec<String>>) -> Option<FrontWindow> {
    let step = |s: &str| {
        stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(s.to_string())
    };
    step("windows");
    let (w, name, bundle) = topmost(src)?;
    if src.excluded(&bundle, &name, None) {
        return Some(FrontWindow {
            app: EXCLUDED.into(),
            origin: origin::PRIVATE.into(),
            withheld: Some(excluded_app_text()),
            ..Default::default()
        });
    }
    let browser = src.is_browser(&bundle);
    if browser {
        step("browser tab");
    }
    let (mut tab, mut tab_note, mut doubt) = if browser {
        read_tab(src, &w, &bundle, false)
    } else {
        (None, None, false)
    };
    let mut image: Option<Vec<u8>> = None;
    let mut withheld: Option<String> = None;
    let mut masking = String::new();
    let mut named: Option<(String, String)> = None;
    step("screenshot");
    match src.shoot(&w).await {
        Err(ShotError::Refused(why)) => withheld = Some(why),
        Err(ShotError::Failed(kind)) => {
            withheld = Some(format!("the window couldn't be captured ({kind})"))
        }
        Ok((raw, m, n)) => {
            masking = m;
            named = n;
            step("image redaction");
            match src.redact_image(&raw) {
                Ok(i) => image = Some(i),
                Err(_) => {
                    withheld =
                        Some("the screenshot couldn't be redacted, so it was withheld".into())
                }
            }
        }
    }
    let mut source = "applescript";
    if let (true, Some((u, t)), false) = (browser, named.clone(), doubt) {
        tab = Some(((!u.is_empty()).then_some(u), (!t.is_empty()).then_some(t)));
        tab_note = None;
        source = "extension";
    } else if browser && tab.is_none() && tab_note.is_none() {
        step("browser tab after the shot");
        (tab, tab_note, doubt) = read_tab(src, &w, &bundle, true);
    }
    let _ = doubt;
    let (url, tab_title) = tab.clone().unwrap_or((None, None));
    let excluded_site = url.is_some() && src.excluded(&bundle, &name, url.as_deref());
    let read: Vec<(String, String)> = match &url {
        Some(u) => vec![("web".into(), u.clone())],
        None => src.read_sources(&bundle),
    };
    let origin = if excluded_site {
        origin::external("web", "excluded")
    } else {
        let parts: Vec<String> = read
            .iter()
            .map(|(k, i)| origin::from_read_source(k, i))
            .collect();
        origin::join(parts.iter().map(String::as_str))
    };
    let opt = |v: &Option<String>| v.clone().map_or(Value::Null, Value::from);
    let mut facts = dict([
        ("app", Value::from(name.clone())),
        ("bundle_id", Value::from(bundle.clone())),
        ("title", Value::from(w.title.clone())),
        (
            "url",
            if excluded_site {
                Value::from(EXCLUDED)
            } else {
                opt(&url)
            },
        ),
        ("tab_title", opt(&tab_title)),
    ]);
    if let Some(note) = tab_note.as_ref().filter(|n| !n.is_empty()) {
        facts.insert("tab_note".into(), Value::from(note.clone()));
    }
    if !masking.is_empty() && image.is_some() {
        facts.insert("masking".into(), Value::from(masking.clone()));
    }
    let facts = src.redact().dict(&facts);
    let mut reference = None;
    if let Some(img) = &image {
        step("keeping the evidence");
        match src.keep(img, &facts, &origin).await {
            Ok(r) => reference = Some(r),
            Err(kind) => {
                image = None;
                withheld = Some(format!(
                    "the screenshot couldn't be kept as evidence ({kind})"
                ));
            }
        }
    }
    let s = |k: &str| text_or_empty(facts.get(k));
    let o = |k: &str| match facts.get(k) {
        Some(Value::Null) | None => None,
        Some(v) => Some(ferb_py::value::py_str(v)),
    };
    Some(FrontWindow {
        app: s("app"),
        bundle_id: s("bundle_id"),
        title: s("title"),
        url: o("url"),
        tab_title: o("tab_title"),
        image_ref: reference,
        origin,
        withheld,
        masking: if image.is_some() {
            masking
        } else {
            String::new()
        },
        image,
        tab_note,
        tab_source: tab.is_some().then(|| source.to_string()),
    })
}
