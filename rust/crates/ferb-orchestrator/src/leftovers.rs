//! ferb/orchestrator/leftovers.py: the owner's lines for what a task's cleanup couldn't release (HAND-419), one
//! plain line per leftover. Raw errors stay in the task.cleanup span.

use serde_json::Value;

use ferb_py::value::py_str;

pub const UNSAID: &str =
    "I couldn't finish tidying up after this task. The details are in the task's record.";
pub const OPENED_TAB: &str =
    "I couldn't close one tab I opened: {where}. Close it if it's still open.";
pub const OWNERS_TAB: &str = "I couldn't fully let go of one of your tabs: {where}. It's still open as you left it; if {browser} says FERB started debugging it, press Cancel there.";

/// leftovers.py:20 `BY_RESOURCE`.
pub fn by_resource(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "browser" | "ferb_browser" => "I couldn't confirm that the browser tabs this task used were closed or let go of. Close any FERB tab group that's still open.",
        "app" => "I couldn't confirm that the apps this task opened were closed. Quit any you don't need.",
        "shell" => "I couldn't confirm that this task's terminal commands all stopped. Activity Monitor shows anything still running.",
        "listen" => "I couldn't confirm that listening stopped when this task ended.",
        "recording" => "A screen recording this task made didn't end cleanly. The details are in the task's record.",
        _ => return None,
    })
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// `str(x or "")`.
fn text_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(x) if ferb_py::value::truthy(x) => py_str(x),
        _ => String::new(),
    }
}

/// leftovers.py:31 `leftover_lines`.
pub fn leftover_lines(resources: &[Value], errors: &[String]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    for r in resources {
        let Some(r) = r.as_object() else { continue };
        if !truthy(r.get("error")) {
            continue;
        }
        said.push(py_str(&r["error"]));
        let kind = text_or_empty(r.get("resource"));
        if kind == "app" && truthy(r.get("left_running")) {
            continue;
        }
        let tabs: Vec<&serde_json::Map<String, Value>> = match r.get("tabs") {
            Some(Value::Array(items)) => items.iter().filter_map(|t| t.as_object()).collect(),
            _ => Vec::new(),
        };
        if tabs.is_empty() {
            lines.push(by_resource(&kind).unwrap_or(UNSAID).to_string());
        } else {
            lines.extend(tabs.into_iter().map(tab_line));
        }
    }
    if errors.iter().any(|e| !said.contains(e)) {
        lines.push(UNSAID.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

fn tab_line(tab: &serde_json::Map<String, Value>) -> String {
    let browser = {
        let b = text_or_empty(tab.get("browser"));
        if b.is_empty() {
            "the browser".to_string()
        } else {
            b
        }
    };
    let profile = text_or_empty(tab.get("profile"));
    let title = text_or_empty(tab.get("title"));
    let account = text_or_empty(tab.get("account"));
    let mut place = browser.clone();
    if !profile.is_empty() {
        place.push_str(&format!(" \u{201c}{profile}\u{201d}"));
    }
    if !profile.is_empty() && !account.is_empty() {
        place.push_str(&format!(" ({account})"));
    }
    if !title.is_empty() {
        place.push_str(&format!(", {title}"));
    }
    let template = if truthy(tab.get("borrowed")) {
        OWNERS_TAB
    } else {
        OPENED_TAB
    };
    crate::py::format_named(template, &[("where", &place), ("browser", &browser)])
}
