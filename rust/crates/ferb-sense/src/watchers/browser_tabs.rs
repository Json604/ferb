//! Browser tabs (`ferb/sense/watchers/browser_tabs.py`): page loads and tab switches as metadata. Paired profiles
//! report through the extension (`tab_observation`); Safari and the Chromium browsers with tab scripting are read by
//! AppleScript only while frontmost and only with Automation already granted (never asked for here). The front
//! window at send reads every window and takes the active tab of the captured one (`window_tab`), or none. Never
//! selects a tab, profile or account.

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_sanitize::parity::hostname;
use serde_json::Value;

use super::base::{poll, ReadFocused};
use crate::batches::BROWSER_BUNDLES;
use crate::front_window::TabLookup;
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Dict, Observation};

/// browser_tabs.py:25: a tab switched to, a page finished loading.
pub const RECORDED: [&str; 2] = ["activated", "updated"];

/// browser_tabs.py:28 `tab_observation`. A URL `urlsplit` refuses gives nothing (Python raised out of the hub's
/// callback, so nothing was recorded either).
#[allow(clippy::too_many_arguments)]
pub fn tab_observation(
    browser: &str,
    folder: &str,
    ferb_identity: bool,
    event: &str,
    url: Option<&str>,
    title: Option<&str>,
    incognito: Option<bool>,
    tab_task: Option<&str>,
) -> Option<Observation> {
    let url = url.filter(|u| !u.is_empty())?;
    if !RECORDED.contains(&event) {
        return None;
    }
    let host = hostname(url).ok()?.unwrap_or_default();
    let mut hints = Dict::new();
    hints.insert("ferb_identity".into(), Value::Bool(ferb_identity));
    hints.insert("tab_task".into(), tab_task.map_or(Value::Null, Value::from));
    Some(Observation::with_hints(
        "browser_tab",
        dict([
            ("browser", Value::from(browser)),
            ("profile", Value::from(folder)),
            ("host", Value::from(host)),
            ("url", Value::from(url)),
            ("title", Value::from(title.unwrap_or(""))),
            ("incognito", Value::Bool(incognito.unwrap_or(false))),
        ]),
        hints,
    ))
}

const CHROMIUM_TAB: &str =
    "set t to active tab of front window\nreturn (URL of t) & linefeed & (title of t)";
const SAFARI_TAB: &str =
    "set t to current tab of front window\nreturn (URL of t) & linefeed & (name of t)";

/// browser_tabs.py:41 `SCRIPTS`: (bundle id, browser key, front-tab script body), Safari first, then every Chromium
/// browser with tab scripting (Opera's dictionary has none).
pub fn scripts() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut out = vec![("com.apple.Safari", "safari", SAFARI_TAB)];
    for (key, bundle) in BROWSER_BUNDLES {
        if key != "opera" && key != "safari" {
            out.push((bundle, key, CHROMIUM_TAB));
        }
    }
    out
}

fn script_of(bundle: &str) -> Option<(&'static str, &'static str, &'static str)> {
    scripts().into_iter().find(|(b, _, _)| *b == bundle)
}

/// browser_tabs.py:47: the front tab script for this browser.
pub fn front_tab_script(bundle: &str) -> Option<String> {
    let (_, _, body) = script_of(bundle)?;
    Some(format!(
        "tell application id \"{bundle}\"\n{body}\nend tell"
    ))
}

/// browser_tabs.py:51: the front-tab script's answer, URL then title.
pub fn parse_front_tab(stdout: &str) -> Option<(String, String)> {
    let s = stdout.strip_suffix('\n').unwrap_or(stdout);
    let (url, title) = s.split_once('\n').unwrap_or((s, ""));
    let url = ferb_py::value::strip(url);
    (!url.is_empty()).then(|| (url.to_string(), title.to_string()))
}

fn tab_part(tab: &str, title: &str) -> String {
    format!(
        "set tb to {tab} of w\n      set v to URL of tb\n      if v is not missing value then set u to v\n      set v to {title} of tb\n      if v is not missing value then set ti to v"
    )
}

/// browser_tabs.py:69 `_WINDOWS` formatted for one browser: every window's bounds, title and active tab.
pub fn windows_script(bundle: &str) -> Option<String> {
    let (_, key, _) = script_of(bundle)?;
    let tab = if key == "safari" {
        tab_part("current tab", "name")
    } else {
        tab_part("active tab", "title")
    };
    Some(format!(
        "set fs to character id 31\nset rs to character id 30\nset gs to character id 29\ntell application id \"{bundle}\"\n  set out to \"\"\n  repeat with w in (get windows)\n    set b to \"\"\n    try\n      set {{x1, y1, x2, y2}} to bounds of w\n      set b to (x1 as text) & gs & (y1 as text) & gs & (x2 as text) & gs & (y2 as text)\n    end try\n    set n to \"\"\n    try\n      set v to name of w\n      if v is not missing value then set n to v\n    end try\n    set u to \"\"\n    set ti to \"\"\n    try\n      {tab}\n    end try\n    set out to out & b & fs & n & fs & u & fs & ti & rs\n  end repeat\n  return out\nend tell"
    ))
}

/// browser_tabs.py:95.
pub const NO_WINDOW: &str = "none of the browser's windows is where the captured window is, so FERB can't tell which tab it shows: no URL or tab title";
pub const AMBIGUOUS: &str = "more than one of the browser's windows could be the captured one and its title doesn't single one out, so FERB can't tell which tab it shows: no URL or tab title";

/// browser_tabs.py:106 `ScriptedWindow`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptedWindow {
    pub bounds: Option<(f64, f64, f64, f64)>,
    pub name: String,
    pub url: String,
    pub title: String,
}

/// Python's `float(text)` for the numbers AppleScript writes.
fn py_float(s: &str) -> Option<f64> {
    let t = ferb_py::value::strip(s);
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    if matches!(body, "inf" | "infinity" | "nan") {
        return t.parse::<f64>().ok().or(Some(if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }));
    }
    if !t
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    t.parse().ok()
}

/// browser_tabs.py:115 `_bounds`: (x, y, w, h) from AppleScript's left, top, right, bottom.
pub fn bounds(field: &str) -> Option<(f64, f64, f64, f64)> {
    let parts: Vec<f64> = field
        .split('\u{1d}')
        .map(|v| py_float(&ferb_py::value::strip(v).replace(',', ".")))
        .collect::<Option<_>>()?;
    let [x1, y1, x2, y2] = parts[..] else {
        return None;
    };
    Some((x1, y1, x2 - x1, y2 - y1))
}

/// browser_tabs.py:131: the windows script's answer.
pub fn parse_windows(stdout: &str) -> Vec<ScriptedWindow> {
    let s = stdout.strip_suffix('\n').unwrap_or(stdout);
    s.split('\u{1e}')
        .filter_map(|record| {
            let f: Vec<&str> = record.split('\u{1f}').collect();
            (f.len() == 4).then(|| ScriptedWindow {
                bounds: bounds(f[0]),
                name: f[1].to_string(),
                url: ferb_py::value::strip(f[2]).to_string(),
                title: f[3].to_string(),
            })
        })
        .collect()
}

/// browser_tabs.py:138 `window_tab`: the browser window FERB captured among those its AppleScript lists.
pub fn window_tab(
    windows: &[ScriptedWindow],
    at: (f64, f64, f64, f64),
    title: &str,
) -> Result<ScriptedWindow, TabLookup> {
    let near = |b: (f64, f64, f64, f64)| {
        [b.0 - at.0, b.1 - at.1, b.2 - at.2, b.3 - at.3]
            .iter()
            .all(|d| d.abs() <= 2.0)
    };
    let here: Vec<&ScriptedWindow> = windows
        .iter()
        .filter(|w| w.bounds.is_some_and(near))
        .collect();
    let anywhere: Vec<&ScriptedWindow> = windows.iter().filter(|w| w.bounds.is_none()).collect();
    if here.len() == 1 && anywhere.is_empty() {
        return Ok(here[0].clone());
    }
    if here.is_empty() && anywhere.is_empty() {
        return Err(TabLookup {
            message: NO_WINDOW.into(),
            ambiguous: false,
        });
    }
    let named: Vec<&&ScriptedWindow> = here
        .iter()
        .chain(anywhere.iter())
        .filter(|w| !title.is_empty() && (w.name == title || w.title == title))
        .collect();
    if named.len() != 1 {
        return Err(TabLookup {
            message: AMBIGUOUS.into(),
            ambiguous: true,
        });
    }
    Ok((*named[0]).clone())
}

/// browser_tabs.py:157 `ScriptedTabsWatcher`: the AppleScript read, for browsers the extension doesn't cover.
pub struct ScriptedTabsWatcher {
    granted: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    connected: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub interval_s: f64,
    front: ReadFocused,
    /// the front tab (url, title) of this browser; `Err`: the script couldn't run (skipped)
    read: Arc<dyn Fn(&str) -> Result<Option<(String, String)>, String> + Send + Sync>,
    last: Mutex<Option<(String, String)>>,
}

impl ScriptedTabsWatcher {
    pub fn new(
        granted: Arc<dyn Fn(&str) -> bool + Send + Sync>,
        connected: Arc<dyn Fn(&str) -> bool + Send + Sync>,
        front: ReadFocused,
        read: Arc<dyn Fn(&str) -> Result<Option<(String, String)>, String> + Send + Sync>,
    ) -> ScriptedTabsWatcher {
        ScriptedTabsWatcher {
            granted,
            connected,
            interval_s: 3.0,
            front,
            read,
            last: Mutex::new(None),
        }
    }

    /// browser_tabs.py:168 `check`. `Err`: a failed read (a URL `urlsplit` refuses: Python's ValueError).
    pub fn check(&self) -> Result<Option<Observation>, String> {
        let Some(f) = (self.front)() else {
            return Ok(None);
        };
        let Some((_, browser, _)) = script_of(&f.bundle_id) else {
            return Ok(None);
        };
        if (browser != "safari" && (self.connected)(browser)) || !(self.granted)(&f.bundle_id) {
            return Ok(None);
        }
        let tab = match (self.read)(&f.bundle_id) {
            Ok(Some(t)) => t,
            Ok(None) | Err(_) => return Ok(None),
        };
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_ref() == Some(&(f.bundle_id.clone(), tab.0.clone())) {
            return Ok(None);
        }
        *last = Some((f.bundle_id.clone(), tab.0.clone()));
        let host = hostname(&tab.0)
            .map_err(|_| "ValueError: Invalid URL".to_string())?
            .unwrap_or_default();
        Ok(Some(Observation::new(
            "browser_tab",
            dict([
                ("browser", Value::from(browser)),
                ("profile", Value::Null),
                ("host", Value::from(host)),
                ("url", Value::from(tab.0)),
                ("title", Value::from(tab.1)),
            ]),
        )))
    }
}

impl Watcher for ScriptedTabsWatcher {
    fn source(&self) -> &str {
        "browser_tab"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            *self
                .last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            let me = self.clone();
            poll(
                self.interval_s,
                &stop,
                Arc::new(move || {
                    if let Some(o) = me.check()? {
                        emit(o);
                    }
                    Ok(())
                }),
            )
            .await
        })
    }
}
