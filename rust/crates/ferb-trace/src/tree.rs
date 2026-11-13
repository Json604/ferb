//! What `ferb trace` prints (`ferb/cli/admin.py`: `render_trace`, `_span_text`, `_plain`, `trace`): the story, the
//! span tree (`--full`, plain) and the JSON export. The span store's tree and `outcome` come in as data (the
//! `trace.get` answer); nothing here reads a database.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::py::{
    char_len, g, gd, has, items, loads, obj, or, prefix, rstrip, splitlines, str_of, string, strip,
    text, total_seconds, truthy, EMPTY,
};
use crate::{render, timing};

const ARG_KEYS: [&str; 18] = [
    "url",
    "path",
    "command",
    "text",
    "query",
    "app",
    "bundle_id",
    "action",
    "allow_foreground",
    "element",
    "direction",
    "amount",
    "distance",
    "x",
    "y",
    "profile",
    "tab",
    "id",
];

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;?<>=]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]|\x1b[78=>]|[\x00-\x08\x0e-\x1f]")
        .expect("the screen-control pattern compiles")
});

/// `_clip(s, n, full)`: whitespace folded; cut with "…" past `n` characters unless `full`.
fn clip(s: &Value, n: usize, full: bool) -> String {
    let s = crate::py::squeeze(&str_of(s));
    if full || char_len(&s) <= n {
        s
    } else {
        format!("{}…", prefix(&s, n - 1))
    }
}

/// `_plain(text)`: terminal output as text: screen-control codes removed, blank and repeated lines dropped.
fn plain(input: &str) -> String {
    let stripped = ANSI.replace_all(input, "").replace('\r', "\n");
    let mut seen: Vec<String> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for line in splitlines(&stripped) {
        let t = strip(line);
        let boxes = t.chars().all(|c| "─│╭╮╰╯·✻✽✶✢✳ ".contains(c));
        if char_len(t) > 2 && !boxes && !seen.iter().any(|s| s == t) {
            seen.push(t.to_owned());
            lines.push(t.to_owned());
        }
    }
    lines.join("\n")
}

fn arrow(direction: &Value) -> &'static str {
    match direction.as_str() {
        Some("up") => "↑",
        Some("down") => "↓",
        Some("left") => "←",
        Some("right") => "→",
        _ => "↕",
    }
}

fn dumps(v: &Value) -> String {
    ferb_py::json::dumps_utf8(v)
}

/// `_span_text(node, full)`: (the span's one-line label, the lines under it).
fn span_text(node: &Value, full: bool) -> (String, Vec<String>) {
    let inp = obj(node, "input");
    let out = obj(node, "output");
    let kind = text(node, "kind");
    if full {
        let name = str_of(or(g(inp, "tool"), or(g(inp, "goal"), &string(""))));
        let label = rstrip(&format!("{kind}  {name}")).to_owned();
        let mut details = serde_json::Map::new();
        if let Some(m) = node.as_object() {
            for (k, v) in m {
                if k != "children" {
                    details.insert(k.clone(), v.clone());
                }
            }
        }
        let pretty = ferb_py::json::dumps_with(
            &Value::Object(details),
            ferb_py::json::Style {
                ascii: false,
                sort_keys: false,
                indent: Some(2),
            },
        );
        return (
            label,
            splitlines(&pretty).into_iter().map(str::to_owned).collect(),
        );
    }
    if kind.starts_with("disturbance.") {
        if kind == "disturbance.change" {
            let actions: Vec<String> = items(out, "actions")
                .iter()
                .map(|a| format!("{} ({})", str_of(g(a, "tool")), str_of(g(a, "span_id"))))
                .collect();
            let actions = if actions.is_empty() {
                "no overlapping action".to_owned()
            } else {
                actions.join(", ")
            };
            let changes = gd(out, "changes", &EMPTY);
            let names: Vec<String> = match changes {
                Value::Object(m) => m.keys().cloned().collect(),
                Value::Array(a) => a.iter().map(str_of).collect(),
                _ => Vec::new(),
            };
            return (
                format!("desktop changed: {}", names.join(", ")),
                vec![
                    format!(
                        "Actions near change: {actions}; cause and authorization not inferred."
                    ),
                    format!("→ {}", dumps(changes)),
                ],
            );
        }
        return (kind.to_owned(), vec![format!("→ {}", dumps(out))]);
    }
    if kind == "tool.call" {
        let args = obj(inp, "args");
        let shown: Vec<String> = ARG_KEYS
            .iter()
            .filter(|k| {
                let v = g(args, k);
                !(v.is_null() || v.as_str() == Some(""))
            })
            .map(|k| format!("{k}={}", clip(g(args, k), 120, full)))
            .collect();
        let mut label = format!(
            "{}  {}",
            str_of(gd(inp, "tool", &string("?"))),
            shown.join("  ")
        );
        if g(args, "action").as_str() == Some("scroll") {
            label.push_str(&format!(
                "  {}",
                arrow(gd(args, "direction", &string("down")))
            ));
        }
        if truthy(g(inp, "why")) {
            label.push_str(&format!("  (why: {})", clip(g(inp, "why"), 100, full)));
        }
        if truthy(g(node, "error")) {
            return (
                label,
                vec![format!("✗ {}", clip(g(node, "error"), 300, full))],
            );
        }
        let body = g(out, "output");
        if let Some(b) = body.as_str() {
            let lines: Vec<String> = splitlines(&plain(b))
                .into_iter()
                .map(str::to_owned)
                .collect();
            let shown: &[String] = if full || lines.len() <= 6 {
                &lines
            } else {
                &lines[lines.len() - 6..]
            };
            return (label, shown.iter().map(|l| format!("→ {l}")).collect());
        }
        let keys = [
            "title", "url", "error", "detail", "opened", "tab", "matches", "met", "path",
        ];
        let brief: Vec<String> = keys
            .iter()
            .filter(|k| has(out, k))
            .map(|k| format!("{k}={}", clip(g(out, k), 100, full)))
            .collect();
        return (
            label,
            if brief.is_empty() {
                Vec::new()
            } else {
                vec![format!("→ {}", brief.join("  "))]
            },
        );
    }
    let goal = or(g(inp, "goal"), or(g(inp, "tool"), &EMPTY));
    let goal = if truthy(goal) {
        goal.clone()
    } else {
        string("")
    };
    let label = format!("{kind}  {}", clip(&goal, 140, full));
    let mut lines: Vec<String> = Vec::new();
    if truthy(g(node, "error")) {
        lines.push(format!("✗ {}", clip(g(node, "error"), 300, full)));
    }
    if let Some(result) = g(out, "result").as_str() {
        // Deliberate difference from Python (ENG-6): it raises AttributeError on a result that is JSON but not an object (a number, a
        // list, null, a string); the Rust port shows it as it shows text that is no JSON.
        match loads(result) {
            Some(r) if r.is_object() => {
                let blank = string("");
                let head = format!(
                    "→ {}: {}",
                    str_of(gd(&r, "status", &blank)),
                    clip(gd(&r, "summary", &blank), 400, full)
                );
                lines.push(head.trim_end_matches([':', ' ']).to_owned());
            }
            _ => lines.push(format!("→ {}", clip(&string(result), 300, full))),
        }
    }
    (label, lines)
}

/// `render_trace(result, full)`: a task and all its steps.
pub fn render_trace(result: &Value, full: bool) -> Vec<String> {
    let spans = items(result, "spans");
    if spans.is_empty() {
        return vec![format!(
            "no spans for task {}",
            str_of(g(result, "task_id"))
        )];
    }
    let mut lines = vec![format!("task {}", str_of(g(result, "task_id")))];
    fn walk(node: &Value, depth: usize, full: bool, lines: &mut Vec<String>) {
        let started = text(node, "started_at");
        let dur = match node
            .get("ended_at")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(end) => format!("{:.1}s", total_seconds(started, end).unwrap_or(0.0)),
            None => "running".to_owned(),
        };
        let (label, under) = span_text(node, full);
        let pad = "  ".repeat(depth);
        lines.push(format!(
            "{pad}{}  {label}  [{dur}]",
            crate::py::hms(started)
        ));
        lines.extend(under.into_iter().map(|u| format!("{pad}    {u}")));
        for child in items(node, "children") {
            walk(child, depth + 1, full, lines);
        }
    }
    for root in spans {
        walk(root, 0, full, &mut lines);
    }
    lines
}

/// How `ferb trace` shows a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The story (the default).
    Story,
    /// `--full`: every span's recorded fields, then where the time went.
    Full,
    /// `--json`: the whole answer with the timing breakdown added.
    Json,
}

/// What `ferb trace` writes for a `trace.get` answer, byte for byte (each line ends with a newline).
pub fn trace_output(result: &Value, mode: Mode) -> String {
    let spans: &[Value] = items(result, "spans");
    let breakdown = timing::breakdown(spans);
    let lines: Vec<String> = match mode {
        Mode::Json => {
            let mut whole = result.clone();
            if let Some(m) = whole.as_object_mut() {
                m.insert("timing".to_owned(), Value::Array(breakdown));
            }
            let pretty = ferb_py::json::dumps_with(
                &whole,
                ferb_py::json::Style {
                    ascii: false,
                    sort_keys: false,
                    indent: Some(2),
                },
            );
            return pretty + "\n";
        }
        Mode::Full => {
            let mut l = render_trace(result, true);
            l.extend(timing::lines(&breakdown));
            l
        }
        Mode::Story => {
            let episode = g(result, "episode");
            if episode.is_null() {
                render_trace(result, false)
            } else {
                render::story(episode, g(obj(result, "outcome"), "owner_verdict"))
            }
        }
    };
    let mut out = String::new();
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}
