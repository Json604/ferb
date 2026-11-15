//! System-wide Accessibility event subscriptions (`ferb/sense/watchers/accessibility.py`; decisions: SENSE-1,
//! SENSE-5, SENSE-403). What each event records: no screenshot, input, activation or per-app inference; never an
//! accessibility value; a secure or password field gives neither its title nor its value changes. The observers
//! themselves run on their own CFRunLoop thread in [`crate::mac`]; FERB's own apps get none (rule 17).

use serde_json::Value;

use crate::capture::password_element;
use crate::observation::{dict, Dict, Observation, Redact};

/// accessibility.py:12: notifications on each app's root element.
pub const APP_EVENTS: [&str; 6] = [
    "AXWindowCreated",
    "AXFocusedWindowChanged",
    "AXFocusedUIElementChanged",
    "AXApplicationActivated",
    "AXApplicationDeactivated",
    "AXMainWindowChanged",
];
/// accessibility.py:14: notifications on the focused element and the windows.
pub const ELEMENT_EVENTS: [&str; 4] = [
    "AXValueChanged",
    "AXTitleChanged",
    "AXUIElementDestroyed",
    "AXSelectedChildrenChanged",
];

/// accessibility.py:17: one AX event as recorded. In a browser no title is kept (a page or window title can't be
/// checked against incognito or the excluded domains).
pub fn accessibility_observation(
    bundle_id: &str,
    app: &str,
    pid: i64,
    notification: &str,
    facts: &Dict,
    redact: &dyn Redact,
    is_browser: bool,
) -> Observation {
    let keys: &[&str] = if is_browser {
        &["window_id", "field_id", "role"]
    } else {
        &["window_id", "field_id", "role", "title"]
    };
    let mut data = dict([
        ("bundle_id", Value::from(bundle_id)),
        ("app", Value::from(app)),
        ("pid", Value::from(pid)),
        ("notification", Value::from(notification)),
    ]);
    for k in keys {
        if let Some(v) = facts.get(*k) {
            data.insert(k.to_string(), v.clone());
        }
    }
    Observation::by("accessibility", redact.dict(&data), "unknown")
}

/// An Accessibility element as the event mapping reads it (one attribute at a time).
pub trait AxElement {
    /// An attribute's value as a scalar (text, number or bool); None when absent.
    fn attr(&self, name: &str) -> Option<Value>;
    /// `AXWindow`: the element's window.
    fn window(&self) -> Option<Box<dyn AxElement + '_>>;
    /// Python's `hash(element)` (CFHash): the stand-in id when `AXIdentifier` / `AXWindowNumber` is absent.
    fn hash_id(&self) -> String;
}

fn text(v: Option<Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => String::new(),
    }
}

/// accessibility.py:28 `element_facts`: what an event about `element` records, or None (a value change of a secure
/// or password field: a shown password is a plain AXTextField whose metadata still names it).
pub fn element_facts(kind: &str, element: &dyn AxElement) -> Option<Dict> {
    let role = text(element.attr("AXRole"));
    let subrole = text(element.attr("AXSubrole"));
    let attr = |name: &str| match element.attr(name) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    };
    let hidden = format!("{role}{subrole}").contains("Secure") || password_element(&role, &attr);
    if hidden && kind == "AXValueChanged" {
        return None;
    }
    let field_id = match element.attr("AXIdentifier") {
        Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
        _ => element.hash_id(),
    };
    let window_id = match element.window() {
        None => String::new(),
        Some(w) => match w.attr("AXWindowNumber") {
            Some(v) if ferb_py::value::truthy(&v) => ferb_py::value::py_str(&v),
            _ => w.hash_id(),
        },
    };
    let mut facts = dict([
        ("role", Value::from(role)),
        ("field_id", Value::from(field_id)),
        ("window_id", Value::from(window_id)),
    ]);
    if !hidden {
        facts.insert("title".into(), Value::from(text(element.attr("AXTitle"))));
    }
    Some(facts)
}

/// accessibility.py:116: an app's observer couldn't be made.
pub fn subscription_unavailable(
    bundle_id: &str,
    pid: i64,
    ax_error: i64,
    redact: &dyn Redact,
) -> Observation {
    Observation::by(
        "accessibility",
        redact.dict(&dict([
            ("bundle_id", Value::from(bundle_id)),
            ("pid", Value::from(pid)),
            ("subscription", Value::from("unavailable")),
            ("ax_error", Value::from(ax_error)),
        ])),
        "unknown",
    )
}

/// accessibility.py:129: an app's observer is attached; `unsupported` lists the notifications it refused.
pub fn subscription_attached(
    bundle_id: &str,
    pid: i64,
    unsupported: &[(String, i64)],
    redact: &dyn Redact,
) -> Observation {
    let list: Vec<Value> = unsupported
        .iter()
        .map(|(e, code)| {
            Value::Object(dict([
                ("event", Value::from(e.as_str())),
                ("ax_error", Value::from(*code)),
            ]))
        })
        .collect();
    Observation::by(
        "accessibility",
        redact.dict(&dict([
            ("bundle_id", Value::from(bundle_id)),
            ("pid", Value::from(pid)),
            ("subscription", Value::from("attached")),
            ("unsupported", Value::Array(list)),
        ])),
        "unknown",
    )
}

/// accessibility.py:57: the watcher's failure text (redacted: an element's exception may hold private words).
pub fn subscription_failure(error: &str, redact: &dyn Redact) -> String {
    let d = redact.dict(&dict([("error", Value::from(error))]));
    format!(
        "RuntimeError: Accessibility subscription failed: {}",
        crate::observation::text_or_empty(d.get("error"))
    )
}
