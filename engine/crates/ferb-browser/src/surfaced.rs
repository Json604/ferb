//! What brought a browser window forward during a task, as the extension saw it (HAND-533, extension 0.19.11), and
//! the worker's fact about it. Chrome shows a window by itself for what a page does in it (a tab or window the page
//! opens, a dialog); the extension puts FERB's own windows back down at once and never brings the owner's app back
//! (HAND-212). A page's own request to come forward (focus, fullscreen, picture-in-picture) is refused in the page
//! (`prevented`). Facts only: what happened, never why the page did it.

use serde_json::{Map, Value};

/// The most facts kept per tab until its next result.
pub const KEPT: usize = 10;

const WINDOWS: [&str; 3] = ["ferb", "popup", "owner"];
const CAUSES: [&str; 6] = [
    "new_tab",
    "popup_window",
    "dialog",
    "focus",
    "fullscreen",
    "picture_in_picture",
];

/// One fact from a reply's `surfaced` list, cut to its known fields; `None` when it isn't one.
pub fn fact(v: &Value) -> Option<Map<String, Value>> {
    let o = v.as_object()?;
    let cause = o.get("cause").and_then(Value::as_str)?;
    if !CAUSES.contains(&cause) {
        return None;
    }
    let mut out = Map::new();
    out.insert("cause".into(), cause.into());
    if let Some(w) = o.get("window").and_then(Value::as_str) {
        if !WINDOWS.contains(&w) {
            return None;
        }
        out.insert("window".into(), w.into());
    }
    for k in ["focused", "put_back", "prevented"] {
        if let Some(b) = o.get(k).and_then(Value::as_bool) {
            out.insert(k.into(), b.into());
        }
    }
    let prevented = out.get("prevented") == Some(&Value::Bool(true));
    (prevented || out.contains_key("window")).then_some(out)
}

/// HAND-546: whether a fact is a window that came up (one of FERB's, a popup it claimed, or the owner's), not a request
/// the page made and the extension refused.
pub fn came_up(f: &Map<String, Value>) -> bool {
    f.get("prevented") != Some(&Value::Bool(true)) && f.contains_key("window")
}

/// The worker's line for one fact.
pub fn told(f: &Map<String, Value>) -> String {
    let cause = f.get("cause").and_then(Value::as_str).unwrap_or("");
    let flag = |k: &str| f.get(k) == Some(&Value::Bool(true));
    if flag("prevented") {
        let asked = match cause {
            "fullscreen" => "to go full screen",
            "picture_in_picture" => "to open a picture-in-picture window",
            _ => "to bring its window to the front (window.focus)",
        };
        return format!(
            "the page asked {asked}; FERB refused it in the page, so nothing came forward"
        );
    }
    let why = match cause {
        "new_tab" => "the page opened a new tab",
        "popup_window" => "the page opened a window of its own (a popup, as a sign-in does)",
        _ => "the page showed a dialog",
    };
    let which = match f.get("window").and_then(Value::as_str) {
        Some("ferb") => "FERB's minimized browser window",
        Some("popup") => "that window",
        _ => "the owner's browser window holding this tab",
    };
    let focus = if flag("focused") {
        " and took the keyboard focus from the owner's app"
    } else {
        ""
    };
    let after = match (f.get("window").and_then(Value::as_str), flag("put_back")) {
        (Some("owner"), _) => "FERB leaves the owner's windows as they are",
        (_, true) => "FERB minimized it again at once",
        _ => "FERB couldn't minimize it again",
    };
    // HAND-570: whether FERB put the owner's app back (the daemon's answer, beside the extension's facts)
    let text = |k: &str| f.get(k).and_then(Value::as_str).filter(|t| !t.is_empty());
    let previous = text("previous").unwrap_or("the owner's app");
    let back = match f.get("app_put_back").and_then(Value::as_bool) {
        Some(true) => format!("FERB put {previous} back in front"),
        Some(false) => format!(
            "FERB couldn't put {previous} back in front ({})",
            text("app_why").unwrap_or("unknown")
        ),
        None => "FERB doesn't switch the owner back to their app".to_string(),
    };
    format!("{why}, and the browser brought {which} to the front{focus}; {after}. {back}")
}

/// HAND-649 (extension 0.19.13): an open's `window_made`, when making FERB's minimized window in a profile with none
/// wasn't plain: macOS put it in full screen on a Space of its own first (the owner's own window was full screen in
/// that browser), or it didn't end minimized. Cut to its known fields; `None` when it isn't one.
pub fn window_made(v: Option<&Value>) -> Option<Map<String, Value>> {
    let o = v?.as_object()?;
    let fullscreen = o.get("fullscreen").and_then(Value::as_bool)?;
    let minimized = o.get("minimized").and_then(Value::as_bool)?;
    let mut out = Map::new();
    out.insert("fullscreen".into(), fullscreen.into());
    out.insert("minimized".into(), minimized.into());
    if let Some(n) = o
        .get("tries")
        .and_then(Value::as_i64)
        .filter(|n| (0..=2).contains(n))
    {
        out.insert("tries".into(), n.into());
    }
    Some(out)
}

/// The worker's line for [`window_made`].
pub fn window_made_told(f: &Map<String, Value>) -> String {
    let flag = |k: &str| f.get(k) == Some(&Value::Bool(true));
    let first = if flag("fullscreen") {
        "this profile had no window, so FERB made its own minimized one; macOS first opened it full screen on a \
         Space of its own (the owner's screen may have switched Space for a moment)"
    } else {
        "this profile had no window, so FERB made its own minimized one"
    };
    let after = if flag("minimized") {
        "it is minimized now, and later tabs in this profile open in it"
    } else {
        "FERB couldn't minimize it, so it may be showing on the owner's desktop"
    };
    format!("{first}; {after}")
}
