//! The one-line summary of a tool result and the step line format (`ferb/trace/lines.py`). Every word comes from the
//! call's own arguments and result.

use serde_json::Value;

use crate::py::{
    char_len, g, gd, has, hms, num, prefix, splitlines, squeeze, str_of, truthy, EMPTY,
};

pub const ARG_KEYS: [&str; 12] = [
    "url",
    "path",
    "command",
    "text",
    "query",
    "app",
    "bundle_id",
    "action",
    "element",
    "profile",
    "tab",
    "id",
];
pub const MAX: usize = 160;

/// `clip(text, n)`: whitespace folded, cut with "…" past `n` characters.
pub fn clip(text: &str, n: usize) -> String {
    let s = squeeze(text);
    if char_len(&s) <= n {
        s
    } else {
        format!("{}…", prefix(&s, n.saturating_sub(1)))
    }
}

/// `clip(value, n)` of any recorded value (`str()` of it first).
pub fn clip_value(value: &Value, n: usize) -> String {
    clip(&str_of(value), n)
}

/// `shorten(text, n, fold)`: a short form that never cuts a word (ORCH-601).
pub fn shorten(text: &str, n: usize, fold: bool) -> String {
    let s = if fold { squeeze(text) } else { text.to_owned() };
    if char_len(&s) <= n {
        return s;
    }
    let head: Vec<char> = s.chars().take(n.saturating_sub(1)).collect();
    let at = |i: usize| head[i];
    let mut last_end = None;
    let mut i = 0;
    while i + 1 < head.len() {
        if matches!(at(i), '.' | '!' | '?') && crate::py::is_py_space(at(i + 1)) {
            last_end = Some(i);
            i += 2;
        } else {
            i += 1;
        }
    }
    if let Some(e) = last_end {
        if e + 1 >= n / 2 {
            let cut: String = head[..=e].iter().collect();
            return format!("{cut} …");
        }
    }
    let last_space = head.iter().rposition(|&c| crate::py::is_py_space(c));
    match last_space {
        Some(sp) if sp > 0 => {
            let cut: String = head[..sp].iter().collect();
            format!(
                "{}…",
                cut.trim_end_matches([' ', ',', ';', ':', '(', '—', '-', '\n', '\t'])
            )
        }
        _ => format!("{}…", head.iter().collect::<String>()),
    }
}

/// `plural(n, one, many)`.
pub fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `choices(options)`: "Yes or No", "A, B or C".
pub fn choices(options: &[Value]) -> String {
    let opts: Vec<String> = options.iter().map(str_of).collect();
    if opts.len() < 2 {
        return opts.concat();
    }
    format!(
        "{} or {}",
        opts[..opts.len() - 1].join(", "),
        opts[opts.len() - 1]
    )
}

/// Which check decided each click's point (D31 fix round 4).
pub const POINT_CHECKS: [(&str, &str); 3] = [
    ("hit_test", "hit test"),
    ("window_list", "window list"),
    ("refused: cant_tell", "refused (can't tell)"),
];

/// `point_checks(counts)`: "Click point checks: hit test 3, window list 0, ...".
pub fn point_checks(counts: &Value) -> String {
    let zero = Value::from(0);
    let parts: Vec<String> = POINT_CHECKS
        .iter()
        .map(|(key, label)| format!("{label} {}", str_of(gd(counts, key, &zero))))
        .collect();
    format!("Click point checks: {}", parts.join(", "))
}

/// `duration(seconds)`: "?s", "45s", "2m05s".
pub fn duration(seconds: Option<f64>) -> String {
    match seconds {
        None => "?s".to_owned(),
        Some(x) => {
            let s = crate::py::round_int(x);
            if s >= 60 {
                format!("{}m{:02}s", s.div_euclid(60), s.rem_euclid(60))
            } else {
                format!("{s}s")
            }
        }
    }
}

/// `duration` of a recorded value (`None` unless a number).
pub fn duration_value(v: &Value) -> String {
    duration(if crate::py::is_number(v) {
        Some(num(v))
    } else {
        None
    })
}

/// `front_pid(app)`: a front-app reading's pid; `None` when it couldn't be read.
pub fn front_pid(app: &Value) -> Option<i64> {
    let pid = g(app, "pid");
    crate::py::is_int(pid).then(|| crate::py::int_val(pid))
}

/// What came forward in one task's desktop records, read in order (P26).
#[derive(Default)]
pub struct CameForward {
    pub pid: Option<i64>,
    pub epoch: Option<i64>,
}

impl CameForward {
    pub fn new() -> Self {
        CameForward::default()
    }

    pub fn baseline(&mut self, snapshot: &Value) {
        self.pid = front_pid(g(snapshot, "frontmost_app"));
        let epoch = g(snapshot, "active_space_epoch");
        self.epoch = if crate::py::is_int(epoch) {
            Some(crate::py::int_val(epoch))
        } else {
            None
        };
    }

    /// What one disturbance.change brought forward, "" for nothing.
    pub fn change(&mut self, changes: &Value) -> String {
        let mut parts: Vec<String> = Vec::new();
        if has(changes, "frontmost_app") {
            let entry = crate::py::or(g(changes, "frontmost_app"), &EMPTY);
            if let Some(before) = front_pid(g(entry, "before")) {
                self.pid = Some(before);
            }
            let after = g(entry, "after");
            if let Some(pid) = front_pid(after) {
                if after.is_object() {
                    if self.pid.is_some_and(|p| p != pid) {
                        let fallback = Value::String("another app".into());
                        let name = crate::py::or(
                            g(after, "name"),
                            crate::py::or(g(after, "bundle_id"), &fallback),
                        );
                        parts.push(format!("{} came to the front", str_of(name)));
                    }
                    self.pid = Some(pid);
                }
            }
        }
        if has(changes, "active_space_epoch") {
            let entry = crate::py::or(g(changes, "active_space_epoch"), &EMPTY);
            let (before, after) = (g(entry, "before"), g(entry, "after"));
            if crate::py::is_int(before) {
                self.epoch = Some(crate::py::int_val(before));
            }
            if crate::py::is_int(after) {
                let a = crate::py::int_val(after);
                if self.epoch.is_some_and(|e| e != a) {
                    parts.push("the Space changed".to_owned());
                }
                self.epoch = Some(a);
            }
        }
        parts.join(" and ")
    }
}

/// `seconds(value)`: one decimal under 10 s, whole seconds from 10 s.
pub fn seconds(value: f64) -> String {
    if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{}", value.trunc())
    }
}

/// `no_on_screen_action(...)`: a desktop change with no FERB on-screen call in flight or just ended (P26).
/// `processes` is `null` for a record made before they were kept; `in_task` and `last_ended_s` as recorded.
pub fn no_on_screen_action(
    what: &str,
    processes: &Value,
    task_id: &str,
    in_task: &Value,
    last_ended_s: &Value,
) -> String {
    let running = match processes {
        Value::Null => "not recorded".to_owned(),
        other => {
            let tree = task_id.split('.').next().unwrap_or("");
            let list: Vec<String> = match other {
                Value::Array(a) => a
                    .iter()
                    .map(|p| {
                        let ptask = str_of(gd(p, "task", &Value::String(String::new())));
                        if ptask.split('.').next().unwrap_or("") == tree {
                            format!(
                                "{} (started by this task)",
                                clip_value(gd(p, "command", &Value::String(String::new())), 60)
                            )
                        } else {
                            format!(
                                "{} (started by another FERB task)",
                                str_of(gd(p, "id", &Value::String("?".into())))
                            )
                        }
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let joined = list.join("; ");
            if joined.is_empty() {
                "nothing FERB started".to_owned()
            } else {
                joined
            }
        }
    };
    let last_none = last_ended_s.is_null();
    let when = if in_task.is_null() || (truthy(in_task) && last_none) {
        String::new()
    } else if last_none {
        " (none yet)".to_owned()
    } else if truthy(in_task) {
        format!(" (the last ended {} s earlier)", seconds(num(last_ended_s)))
    } else {
        format!(
            " (none in this task; the last of any FERB task ended {} s earlier)",
            seconds(num(last_ended_s))
        )
    };
    format!("{what} — no FERB on-screen action in progress{when}; running: {running}")
}

fn nonblank_lines(text: &str) -> i64 {
    splitlines(text)
        .iter()
        .filter(|l| !crate::py::strip(l).is_empty())
        .count() as i64
}

/// `len(value)`; `None` for what has none (Python's TypeError).
fn len_of(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => Some(char_len(s) as i64),
        Value::Array(a) => Some(a.len() as i64),
        Value::Object(m) => Some(m.len() as i64),
        _ => None,
    }
}

fn read(c: &Value) -> Option<String> {
    let size = str_of(g(c, "size"));
    let text = str_of(gd(c, "text", &Value::String(String::new())));
    if !truthy(g(c, "offset")) && g(c, "next_offset").is_null() {
        return Some(format!("read {size} bytes"));
    }
    Some(format!(
        "read {} of {size} bytes from offset {}",
        text.len(),
        str_of(gd(c, "offset", &Value::from(0)))
    ))
}

fn list(c: &Value) -> Option<String> {
    let n = len_of(gd(c, "entries", &crate::py::EMPTY_LIST))?;
    let total = g(c, "total");
    if crate::py::is_int(total) && crate::py::int_val(total) > n {
        return Some(format!("{n} of {} entries", str_of(total)));
    }
    Some(plural(n, "entry", "entries"))
}

fn shell(c: &Value) -> Option<String> {
    if truthy(g(c, "running")) {
        return Some(format!("still running ({})", str_of(g(c, "id"))));
    }
    let output = str_of(gd(c, "output", &Value::String(String::new())));
    let mut text = format!(
        "exit {} · {}",
        str_of(g(c, "exit_code")),
        plural(nonblank_lines(&output), "line", "lines")
    );
    if truthy(g(c, "capture_error")) {
        text.push_str(" · output capture failed");
    }
    Some(text)
}

fn ask(a: &Value, c: &Value) -> Option<String> {
    let ferb = if truthy(g(a, "confirm")) {
        " (FERB's question)"
    } else {
        ""
    };
    if truthy(g(c, "declined")) || !has(c, "answer") {
        return Some(format!("no answer{ferb}"));
    }
    let who = if g(c, "answered_by").as_str() == Some("orchestrator") {
        "the orchestrator answered"
    } else {
        "you answered"
    };
    Some(format!(
        "{who}: \"{}\"{ferb}",
        clip_value(g(c, "answer"), 100)
    ))
}

fn rule(tool: &str, a: &Value, c: &Value) -> Option<Option<String>> {
    let empty = Value::String(String::new());
    Some(match tool {
        "fs.list" => list(c),
        "fs.read" => read(c),
        "fs.write" => Some(format!(
            "wrote {} ({} bytes)",
            str_of(gd(c, "path", gd(a, "path", &empty))),
            str_of(gd(c, "bytes", g(c, "size")))
        )),
        "fs.edit" => Some(format!(
            "edited {}",
            str_of(gd(c, "path", gd(a, "path", &empty)))
        )),
        "fs.trash" => Some(format!(
            "moved to Trash: {}",
            str_of(gd(c, "path", gd(a, "path", &empty)))
        )),
        "shell.run" | "shell.wait" => shell(c),
        "shell.send" => Some(format!(
            "sent: \"{}\"",
            clip(&str_of(gd(a, "text", &empty)), 120)
        )),
        "mac.clipboard" => Some(format!(
            "wrote {} characters to clipboard",
            str_of(gd(c, "characters", &Value::from(0)))
        )),
        "owner.ask" => ask(a, c),
        _ => return None,
    })
}

/// `summarize(tool, args, content, is_error)`: one line of what came back.
pub fn summarize(tool: &str, args: &Value, content: &Value, is_error: bool) -> String {
    if is_error && truthy(g(content, "error")) {
        return clip(&format!("error: {}", str_of(g(content, "error"))), MAX);
    }
    if let Some(Some(text)) = rule(tool, args, content) {
        return clip(&text, MAX);
    }
    let mut keys: Vec<&str> = content
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    let joined = keys.join(", ");
    clip(if joined.is_empty() { "done" } else { &joined }, MAX)
}

/// `step_line(...)`: the one line `ferb logs` and the story share.
pub fn step_line(
    at: &str,
    tool: &str,
    args: &Value,
    why: &str,
    summary: &str,
    seconds: Option<f64>,
    error: Option<&str>,
) -> String {
    let shown: Vec<String> = ARG_KEYS
        .iter()
        .filter(|k| {
            let v = g(args, k);
            !(v.is_null() || v.as_str() == Some(""))
        })
        .map(|k| format!("{k}={}", clip_value(g(args, k), 100)))
        .collect();
    let shown = shown.join("  ");
    let mut head = format!("{}  {tool}", hms(at));
    if !shown.is_empty() {
        head.push_str(&format!("  {shown}"));
    }
    if !why.is_empty() {
        head.push_str(&format!("  (why: {})", clip(why, 100)));
    }
    let tail = match error {
        Some(e) if !e.is_empty() => format!("✗ {}", clip(e, 200)),
        _ => format!("→ {summary}"),
    };
    match seconds {
        Some(s) => format!("{head}  {tail}  [{s:.1}s]"),
        None => format!("{head}  {tail}"),
    }
}
