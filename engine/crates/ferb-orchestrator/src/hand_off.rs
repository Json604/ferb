//! The fresh run's hand-off note (ORCH-760, ORCH-719 item 4): built by code from FERB's records, never by a model.
//!
//! When FERB ends a worker's run because its conversation grew past the size, the next run of the same attempt
//! starts from this note: the goal, what is still to achieve (the step's conditions), the latest page, app window
//! and file with the span id that opens each, the actions the ledger hasn't settled, the run's last words, and what
//! the attempt's runs did, one short line per call, newest kept. At most `most` tokens: the oldest lines give way
//! first, counted by tool in one line; then the goal and conditions are shortened. Everything read from the records
//! (places, titles, arguments, results, the run's words) is inside one untrusted-data wrapper; FERB's framing is
//! outside it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::check_evidence::{self, Call, Place};

/// The note's opening (model-facing; ORCH-760).
pub const HEAD: &str = "FERB ended your earlier run of this step because its conversation had grown long, and \
started this one. This is not a new attempt: carry on from where that run stopped. Below is FERB's own record of \
it; don't redo what it did unless a fresh look shows it is needed. Pages, apps and files may have changed since: \
look again before acting on one.";

/// Calls FERB doesn't list (its own reads of the records and of its tools).
const UNLISTED: [&str; 2] = ["task.evidence", "tools.find"];

/// What the note is built from.
pub struct HandOff<'a> {
    pub goal: &'a str,
    /// The step's success conditions, as the planner wrote them.
    pub conditions: &'a [String],
    /// The attempt's recorded calls, in recorded order ([`check_evidence::gather`]).
    pub calls: &'a [Call],
    /// The ledger's actions not settled (intended, executing or unknown), one line each.
    pub unsettled: &'a [String],
    /// The ended run's latest words (ORCH-640), redacted.
    pub last_words: Option<&'a str>,
}

/// A text on one line, at most `most` characters (an ellipsis when cut).
fn one_line(text: &str, most: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= most {
        flat
    } else {
        format!(
            "{}…",
            flat.chars()
                .take(most.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .filter(|x| ferb_py::value::truthy(x) && (x.is_string() || x.is_number()))
        .map(ferb_py::value::py_str)
}

fn tool(call: &Call) -> String {
    call.row
        .get("input")
        .and_then(|i| i.get("tool"))
        .map(crate::py::str_of)
        .unwrap_or_default()
}

/// The arguments that only say where (the place shows them).
const WHERE: [&str; 8] = [
    "tab",
    "url",
    "app",
    "bundle_id",
    "window",
    "path",
    "id",
    "profile",
];

/// One call's line: number, time, tool, where, and a short fact (a failure; a look's title; an action's arguments).
fn line(n: usize, call: &Call, place: &Place, observes: &dyn Fn(&str) -> bool) -> String {
    let row = &call.row;
    let name = tool(call);
    let started = row
        .get("started_at")
        .map(crate::py::str_of)
        .unwrap_or_default();
    let time: String = if started.chars().count() >= 19 {
        started.chars().skip(11).take(5).collect()
    } else {
        started
    };
    let out = row.get("output").cloned().unwrap_or(Value::Null);
    let mut facts: Vec<String> = Vec::new();
    if let Some(e) = row.get("error").filter(|e| !e.is_null()) {
        facts.push(format!("failed: {}", one_line(&crate::py::str_of(e), 90)));
    } else if let Some(e) = out.get("error").filter(|e| ferb_py::value::truthy(e)) {
        facts.push(format!("error: {}", one_line(&crate::py::str_of(e), 90)));
    } else if out.is_null() {
        facts.push("no result recorded".into());
    }
    if observes(&name) {
        if let Some(title) = str_field(&out, "title") {
            facts.push(format!("title {}", one_line(&title, 60)));
        }
    } else {
        let rest: Map<String, Value> = row
            .get("input")
            .and_then(|i| i.get("args"))
            .and_then(Value::as_object)
            .map(|a| {
                a.iter()
                    .filter(|(k, _)| !WHERE.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if !rest.is_empty() {
            facts.push(one_line(
                &ferb_py::json::dumps_utf8(&Value::Object(rest)),
                110,
            ));
        }
    }
    let at = if place.shown.is_empty() {
        String::new()
    } else {
        format!(" at {}", one_line(&place.shown, 90))
    };
    let facts = if facts.is_empty() {
        String::new()
    } else {
        format!("; {}", facts.join("; "))
    };
    format!("{n}. {time} {name}{at}{facts}")
}

/// The newest page, app window and file the calls name, each with the span id of the newest look there (else of the
/// newest call there).
fn latest(calls: &[Call], places: &[Place], observes: &dyn Fn(&str) -> bool) -> Vec<String> {
    let newest = |want: &dyn Fn(&Place) -> bool| places.iter().rposition(want);
    let look_at = |i: usize| {
        (0..=i)
            .rev()
            .find(|&j| places[j].shown == places[i].shown && observes(&tool(&calls[j])))
            .unwrap_or(i)
    };
    let page = newest(&|p: &Place| p.url.is_some()).map(look_at).map(|i| {
        let title = str_field(calls[i].row.get("output").unwrap_or(&Value::Null), "title")
            .unwrap_or_default();
        (i, title)
    });
    let app = newest(&|p: &Place| p.shown.starts_with("app ")).map(look_at);
    let file = newest(&|p: &Place| p.path.is_some());
    let id = |i: usize| {
        calls[i]
            .row
            .get("id")
            .map(crate::py::str_of)
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    if let Some((i, title)) = page {
        let title = if title.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", one_line(&title, 80))
        };
        out.push(format!(
            "page: {}{title} (span_id={})",
            one_line(&places[i].shown, 160),
            id(i)
        ));
    }
    if let Some(i) = app {
        let shown = &places[i].shown;
        out.push(format!(
            "app: {} (span_id={})",
            one_line(shown.strip_prefix("app ").unwrap_or(shown), 120),
            id(i)
        ));
    }
    if let Some(i) = file {
        out.push(format!(
            "file: {} (span_id={})",
            one_line(places[i].path.as_deref().unwrap_or_default(), 160),
            id(i)
        ));
    }
    out
}

/// The note, at most `most` tokens as `count` measures them.
pub fn note(
    h: &HandOff<'_>,
    observes: &dyn Fn(&str) -> bool,
    wrap: &dyn Fn(&str) -> String,
    count: &dyn Fn(&str) -> usize,
    most: usize,
) -> String {
    let listed: Vec<Call> = h
        .calls
        .iter()
        .filter(|c| !UNLISTED.contains(&tool(c).as_str()))
        .cloned()
        .collect();
    let places = check_evidence::places(&listed);
    let lines: Vec<String> = listed
        .iter()
        .enumerate()
        .map(|(i, c)| line(i + 1, c, &places[i], observes))
        .collect();
    let latest = latest(&listed, &places, observes);
    // (goal characters, condition characters): shortened only when the rest can't make room
    for (goal_chars, condition_chars) in [(2_000, 300), (600, 120), (200, 60)] {
        let framing = framing(h, goal_chars, condition_chars);
        // the oldest lines give way first; what they did is counted by tool on one line
        let mut first = 0;
        loop {
            let text = compose(h, &framing, &latest, &listed, &lines, first, wrap);
            if count(&text) <= most {
                return text;
            }
            if first >= lines.len() {
                break;
            }
            // drop about a tenth at a time, at least one
            first += (lines.len() - first).div_ceil(10).max(1);
            first = first.min(lines.len());
        }
    }
    // the last resort: the framing alone, cut to fit
    let framing = framing(h, 200, 60);
    let mut chars: Vec<char> = framing.chars().collect();
    while count(&chars.iter().collect::<String>()) > most && !chars.is_empty() {
        let keep = chars.len() * 9 / 10;
        chars.truncate(keep);
    }
    chars.into_iter().collect()
}

/// FERB's own part: the opening, the goal and what is still to achieve.
fn framing(h: &HandOff<'_>, goal_chars: usize, condition_chars: usize) -> String {
    let mut out = format!("{HEAD}\n\nYour goal: {}", one_line(h.goal, goal_chars));
    let conditions: Vec<&String> = h
        .conditions
        .iter()
        .filter(|c| !c.trim().is_empty())
        .take(10)
        .collect();
    if !conditions.is_empty() {
        out.push_str("\n\nStill to achieve (the step's conditions; FERB's check looks at each):");
        for c in conditions {
            out.push_str(&format!("\n- {}", one_line(c, condition_chars)));
        }
    }
    out
}

fn compose(
    h: &HandOff<'_>,
    framing: &str,
    latest: &[String],
    calls: &[Call],
    lines: &[String],
    first: usize,
    wrap: &dyn Fn(&str) -> String,
) -> String {
    let mut data: Vec<String> = Vec::new();
    if !latest.is_empty() {
        data.push("The latest of each:".into());
        data.extend(latest.iter().map(|l| format!("- {l}")));
    }
    if !h.unsettled.is_empty() {
        data.push("Actions not settled (look before trying one again):".into());
        data.extend(
            h.unsettled
                .iter()
                .map(|u| format!("- {}", one_line(u, 160))),
        );
    }
    if let Some(words) = h
        .last_words
        .map(|w| one_line(w, 400))
        .filter(|w| !w.is_empty())
    {
        data.push(format!("The earlier run's last words: {words}"));
    }
    if !lines.is_empty() {
        data.push(format!(
            "What the earlier runs did, oldest first ({} calls):",
            lines.len()
        ));
        if first > 0 {
            let mut by_tool: BTreeMap<String, usize> = BTreeMap::new();
            for c in &calls[..first] {
                *by_tool.entry(tool(c)).or_default() += 1;
            }
            let mut counted: Vec<(String, usize)> = by_tool.into_iter().collect();
            counted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let counted: Vec<String> = counted.iter().map(|(t, n)| format!("{t} ×{n}")).collect();
            data.push(format!(
                "Calls 1-{first}, not listed one by one: {}",
                counted.join(", ")
            ));
        }
        data.extend(lines[first..].iter().cloned());
    }
    if data.is_empty() {
        return framing.to_string();
    }
    format!(
        "{framing}\n\nFrom FERB's records (task_evidence span_id=<id> opens one; task_evidence query=<words> searches them all):\n{}",
        wrap(&data.join("\n"))
    )
}
