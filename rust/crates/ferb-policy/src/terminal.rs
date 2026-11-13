//! policy/terminal.py: which on-screen element is a terminal (D31, rule 10), from the app and the element's own
//! structural Accessibility facts and its ancestors', and the owner's question before FERB types into one. Any
//! input into a terminal asks, whatever it is; GATE-585/586: one Yes covers that terminal app for the rest of the
//! task tree, except a secret, a clipboard item marked secret, a file that runs when opened, and a script for an
//! app that runs code.

use ferb_core::types::TerminalInput;
use serde_json::{json, Map, Value};

use crate::confirm::{key, Need, Values, TERMINAL_APP, TERMINAL_INPUT};
use crate::protection::DEFAULT_TERMINAL_APPS;
use crate::py;

pub const RULE: &str = TERMINAL_INPUT;
pub const UNASKED: &str = "FERB types into a terminal only after the owner's Yes to that exact input (D31): this input \
would reach {why} in {app}, and the call carries no Yes for it, so nothing was typed. Run commands with FERB's shell \
tool; if FERB's sandbox blocks a command you need, request the escape hatch (shell.rules lists what it lifts), which \
asks the owner once";
pub const RETURN_KEYS: &[&str] = &["return", "enter"];
pub const SYSTEM_EVENTS: &str = "com.apple.systemevents";
/// Apps that run code when a script tells them to (bundle id, casefolded → why).
pub const CODE_RUNNERS: &[(&str, &str)] = &[
    (SYSTEM_EVENTS, "System Events, whose keystrokes type into whatever has the keyboard focus"),
    (
        "com.apple.finder",
        "Finder, which opens files with their default apps (a .command runs in Terminal) and copies files with its \
         own access, past FERB's protection lists",
    ),
    ("com.apple.scripteditor2", "Script Editor, which runs scripts"),
    ("com.apple.automator", "Automator, which runs workflows"),
    ("com.apple.shortcuts.events", "Shortcuts Events, which runs shortcuts"),
    ("com.apple.shortcuts", "Shortcuts, which runs shortcuts"),
];
pub const OPENS_FILES: &[&str] = &["com.apple.finder", SYSTEM_EVENTS];
pub const NO_FACTS_HOSTS: &[(&str, &str)] = &[
    ("dev.zed.zed", "Zed"),
    ("dev.zed.zed-preview", "Zed Preview"),
];
pub const RUN_CLASSES: &[&str] = &[
    "codicon-play",
    "codicon-run",
    "codicon-run-all",
    "codicon-run-above",
    "codicon-run-below",
    "codicon-run-errors",
    "codicon-debug-start",
    "codicon-debug-alt",
    "codicon-debug-alt-small",
];
const CONTROLS: &[&str] = &["AXButton", "AXMenuButton", "AXLink", "AXMenuItem"];
pub const EDITOR_APPS: &[&str] = &[
    "com.microsoft.vscode",
    "com.microsoft.vscodeinsiders",
    "com.todesktop.230313mzl4w4u92",
    "com.exafunction.windsurf",
    "com.vscodium",
    "com.visualstudio.code.oss",
    "com.google.android.studio",
    "com.apple.dt.xcode",
    "dev.zed.zed",
    "dev.zed.zed-preview",
    "com.panic.nova",
    "com.sublimetext.4",
];
pub const TERMINAL_CLASSES: &[&str] = &["xterm", "xterm-helper-textarea"];
const LABELS: &[&str] = &["title", "description"];
const KINDS: &[&str] = &["role", "subrole", "role_description"];
const NOT_READ: &[&str] = &["AXWindow", "AXApplication"];
pub const SHOWN: usize = 60;
pub const LABEL_SHOWN: usize = 30;
pub const BLANK_GLYPHS: &[char] = &['\u{3164}', '\u{115f}', '\u{1160}', '\u{ffa0}', '\u{2800}'];

fn terminal_app(bid: &str) -> bool {
    DEFAULT_TERMINAL_APPS.iter().any(|b| py::casefold(b) == bid)
}

fn code_runner(bid: &str) -> Option<&'static str> {
    CODE_RUNNERS
        .iter()
        .find(|(b, _)| *b == bid)
        .map(|(_, why)| *why)
}

/// One element's facts, as accessibility.surface_facts reports them.
pub type Facts = Map<String, Value>;

fn quote(v: &str) -> String {
    let one = py::squeeze(v);
    let shown = if py::char_len(&one) <= SHOWN {
        one
    } else {
        format!("{}…", py::prefix(&one, SHOWN - 1))
    };
    ferb_py::json::dumps_utf8(&shown)
}

/// A word "terminal", "console" or "shell" in a text (re.I), not glued to other ASCII letters.
fn has_word(text: &str, words: &[&str]) -> bool {
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        if start > 0 && py::ci_ascii_letter(chars[start - 1]) {
            continue;
        }
        for w in words {
            let n = w.chars().count();
            if start + n <= chars.len()
                && chars[start..start + n]
                    .iter()
                    .zip(w.chars())
                    .all(|(c, l)| py::ci_eq(*c, l))
                && !chars
                    .get(start + n)
                    .is_some_and(|c| py::ci_ascii_letter(*c))
            {
                return true;
            }
        }
    }
    false
}

const TERMINAL_WORDS: &[&str] = &["terminal", "console", "shell"];

/// `_CAMEL.sub(" ", value)`: a space between a lowercase letter or digit and a capital (ASCII).
fn camel(value: &str) -> String {
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in value.chars() {
        if c.is_ascii_uppercase()
            && prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

fn names(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(s)) if has_word(&camel(s), TERMINAL_WORDS))
}

fn text_of(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

fn classes(f: &Facts) -> Vec<String> {
    match f.get("dom_classes") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|c| py::casefold(&ferb_py::value::py_str(c)))
            .collect(),
        _ => Vec::new(),
    }
}

fn truthy(f: &Facts, k: &str) -> bool {
    f.get(k).is_some_and(ferb_py::value::truthy)
}

/// `can_host`: whether an app can hold a terminal somewhere in it.
pub fn can_host(bundle_id: &str, owner_apps: &[String], shell_app: bool) -> bool {
    let bid = py::casefold(bundle_id);
    terminal_app(&bid)
        || owner_apps.iter().any(|a| py::casefold(a) == bid)
        || shell_app
        || NO_FACTS_HOSTS.iter().any(|(b, _)| *b == bid)
        || editor_app(bundle_id)
}

/// `surface`: why code counts this a terminal, in words for the owner's question and the trace, else None.
pub fn surface(
    bundle_id: &str,
    chain: &[Facts],
    owner_apps: &[String],
    shell_app: bool,
    unread: bool,
) -> Option<String> {
    let bid = py::casefold(bundle_id);
    if terminal_app(&bid) || owner_apps.iter().any(|a| py::casefold(a) == bid) {
        return Some(format!("a terminal app ({bundle_id})"));
    }
    if shell_app {
        return Some(format!(
            "an app whose Info.plist declares it a shell ({bundle_id})"
        ));
    }
    if let Some((_, name)) = NO_FACTS_HOSTS.iter().find(|(b, _)| *b == bid) {
        return Some(format!(
            "{name}, whose terminal panel Accessibility can't show, so any typing there asks"
        ));
    }
    if unread && chain.is_empty() && editor_app(bundle_id) {
        return Some(format!(
            "an editor ({bundle_id}) whose focused element FERB couldn't read: it could be the editor's terminal, so \
             FERB asks rather than type unchecked"
        ));
    }
    let web = chain
        .iter()
        .any(|f| truthy(f, "dom_classes") || truthy(f, "dom_identifier"));
    for f in chain {
        let mut hit: Vec<String> = classes(f)
            .into_iter()
            .filter(|c| TERMINAL_CLASSES.contains(&c.as_str()))
            .collect();
        if !hit.is_empty() {
            hit.sort();
            hit.dedup();
            return Some(format!(
                "a terminal in the page (xterm.js: DOM class {})",
                hit[hit.len() - 1]
            ));
        }
    }
    if web {
        return None;
    }
    for f in chain {
        for k in KINDS {
            if names(f.get(*k)) {
                return Some(format!(
                    "an element whose {} is {}",
                    k.replace('_', " "),
                    quote(&ferb_py::value::py_str(&f[*k]))
                ));
            }
        }
        if names(f.get("identifier")) {
            return Some(format!(
                "an element identified as {}",
                quote(&ferb_py::value::py_str(&f["identifier"]))
            ));
        }
    }
    for f in chain {
        if text_of(f.get("role")).is_some_and(|r| NOT_READ.contains(&r)) {
            continue;
        }
        for k in LABELS {
            if let Some(v) = text_of(f.get(*k)) {
                if has_word(v, TERMINAL_WORDS) {
                    return Some(format!(
                        "an element labelled {} (a label only: FERB asks rather than guess)",
                        quote(v)
                    ));
                }
            }
        }
    }
    None
}

fn size(text: &str) -> String {
    let lines = crate::provenance::splitlines(text).len().max(1);
    format!(
        "{} characters on {lines} line{}",
        py::char_len(text),
        if lines == 1 { "" } else { "s" }
    )
}

/// `structural`: whether one element is a terminal by its structure alone (a label never counts).
pub fn structural(f: &Facts) -> bool {
    if classes(f)
        .iter()
        .any(|c| TERMINAL_CLASSES.contains(&c.as_str()))
    {
        return true;
    }
    if truthy(f, "dom_classes") || truthy(f, "dom_identifier") {
        return false;
    }
    KINDS
        .iter()
        .chain(["identifier"].iter())
        .any(|k| names(f.get(*k)))
}

/// `^\s*(words)\s*$` under re.I (`$` also before a final newline).
fn whole_word(text: &str, words: &[&str]) -> bool {
    let t = py::strip(text);
    words.iter().any(|w| {
        t.chars().count() == w.chars().count()
            && t.chars().zip(w.chars()).all(|(c, l)| py::ci_eq(c, l))
    })
}

/// `run_menu`: a menu path under a menu-bar menu that operates the app's terminal or runs its code.
pub fn run_menu(path: &[String]) -> bool {
    path.first()
        .is_some_and(|p| whole_word(p, &["terminal", "run", "console", "shell"]))
}

/// `TERMINAL_MENUS.match(title)`: a menu that operates the terminal itself.
pub fn terminal_menu(title: &str) -> bool {
    whole_word(title, TERMINAL_WORDS)
}

/// `editor_app` (N6).
pub fn editor_app(bundle_id: &str) -> bool {
    let b = py::casefold(bundle_id);
    EDITOR_APPS.contains(&b.as_str()) || b.starts_with("com.jetbrains.")
}

/// `_RUN_LABEL.match`: `(?i)^\s*(run|debug)(?![a-z])`.
fn run_label(text: &str) -> bool {
    let t: Vec<char> = py::lstrip(text).chars().collect();
    ["run", "debug"].iter().any(|w| {
        let n = w.len();
        t.len() >= n
            && t[..n].iter().zip(w.chars()).all(|(c, l)| py::ci_eq(*c, l))
            && !t.get(n).is_some_and(|c| py::ci_ascii_letter(*c))
    })
}

/// `run_control`: why pressing this control runs code, else None.
pub fn run_control(chain: &[Facts], editor: bool) -> Option<String> {
    let f = chain.first()?;
    let role = text_of(f.get("role"));
    if !role.is_some_and(|r| CONTROLS.contains(&r)) {
        return None;
    }
    let mut hit: Vec<String> = classes(f)
        .into_iter()
        .filter(|c| RUN_CLASSES.contains(&c.as_str()))
        .collect();
    if !hit.is_empty() {
        hit.sort();
        return Some(format!("a control that runs code (class {})", hit[0]));
    }
    if editor && (role == Some("AXMenuItem") || !truthy(f, "dom_classes")) {
        for k in LABELS {
            if let Some(v) = text_of(f.get(*k)) {
                if run_label(v) {
                    return Some(format!(
                        "a control labelled {} (a label only: FERB asks rather than guess)",
                        quote(v)
                    ));
                }
            }
        }
    }
    None
}

fn hidden(c: char, before: Option<char>) -> bool {
    if !py::is_printable(c) || BLANK_GLYPHS.contains(&c) || py::is_cf(c) {
        return true;
    }
    py::is_mn(c) && !before.is_some_and(py::is_letter_or_number)
}

/// `visible`: text as the owner must see it: JSON-quoted (`quote`) and every character that would read as nothing
/// written as a visible escape.
pub fn visible(text: &str, quote: bool) -> String {
    let s = if quote {
        ferb_py::json::dumps_utf8(text)
    } else {
        text.to_string()
    };
    let mut out = String::with_capacity(s.len());
    let mut before: Option<char> = None;
    for c in s.chars() {
        if hidden(c, before) {
            let cp = c as u32;
            out.push_str(&if cp <= 0xFFFF {
                format!("\\u{cp:04x}")
            } else {
                format!("\\U{cp:08x}")
            });
            before = None;
        } else {
            out.push(c);
            before = Some(c);
        }
    }
    out
}

/// `newline`: whether typed text holds its own Return.
pub fn newline(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

/// `app_wide` (GATE-586).
pub fn app_wide(inp: &TerminalInput) -> bool {
    !(inp.secret.as_deref().is_some_and(|s| !s.is_empty())
        || inp.concealed
        || !inp.files.is_empty()
        || code_runner(&py::casefold(&inp.bundle_id)).is_some())
}

/// `app_key` (GATE-586): the key one terminal Yes records for its app in the task tree.
pub fn app_key(bundle_id: &str) -> String {
    key(&[json!(TERMINAL_APP), json!(py::casefold(bundle_id))])
}

fn opt_some(x: &Option<String>) -> Option<&str> {
    x.as_deref().filter(|s| !s.is_empty())
}

/// `need`: the owner's question for one terminal input (a Yes covers this run).
pub fn need(inp: &TerminalInput) -> Need {
    let wide = app_wide(inp);
    let shown = inp
        .text
        .as_deref()
        .map_or(String::new(), |t| visible(t, true));
    let label = py::squeeze(&inp.label);
    let label = if py::char_len(&label) <= LABEL_SHOWN {
        label
    } else {
        format!("{}…", py::rstrip(py::prefix(&label, LABEL_SHOWN - 1)))
    };
    let vis = |x: &Option<String>| opt_some(x).map_or(String::new(), |v| visible(v, true));
    let files: Vec<String> = inp
        .files
        .iter()
        .map(|(p, _, t)| {
            format!(
                "{}, which can run when opened and holds {}",
                visible(p, true),
                visible(t, true)
            )
        })
        .collect();
    let named: Vec<String> = inp
        .named
        .iter()
        .map(|p| format!("{}, which can't run when opened", visible(p, true)))
        .collect();
    let mut names: Vec<String> = inp
        .files
        .iter()
        .map(|(p, _, _)| format!("{} (can run when opened)", visible(p, false)))
        .collect();
    names.extend(
        inp.named
            .iter()
            .map(|p| format!("{} (can't run)", visible(p, false))),
    );
    let yes = |b: bool| if b { "yes" } else { "" };
    let values = Values::of(&[
        ("app", &visible(&inp.app, false)),
        ("surface", &visible(&inp.surface, false)),
        ("text", &shown),
        ("keys", &visible(inp.keys.as_deref().unwrap_or(""), false)),
        (
            "window",
            &(if inp.window.is_empty() {
                String::new()
            } else {
                visible(&inp.window, true)
            }),
        ),
        (
            "label",
            &(if label.is_empty() {
                String::new()
            } else {
                visible(&label, true)
            }),
        ),
        ("menu", &vis(&inp.menu)),
        ("secret", &vis(&inp.secret)),
        ("script", &vis(&inp.script)),
        ("drag", yes(inp.drag)),
        ("runs", yes(inp.runs)),
        ("control", &vis(&inp.control)),
        ("selection", &vis(&inp.selection)),
        (
            "selection_size",
            &opt_some(&inp.selection).map_or(String::new(), size),
        ),
        ("file", &vis(&inp.file)),
        ("runs_where", &visible(&inp.runs_where, false)),
        ("files", &files.join("; ")),
        ("named", &named.join("; ")),
        ("names", &names.join("; ")),
        (
            "open_path",
            &inp.files
                .first()
                .map_or(String::new(), |f| visible(&f.0, true)),
        ),
        (
            "open_text",
            &inp.files
                .first()
                .map_or(String::new(), |f| visible(&f.2, true)),
        ),
        ("paste", yes(inp.paste)),
        ("concealed", yes(inp.concealed)),
        ("enter", yes(inp.enter)),
        ("opens", yes(inp.opens)),
        ("wide", yes(wide)),
    ]);
    let o = |x: &Option<String>| x.as_ref().map_or(Value::Null, |v| json!(v));
    let files_key: Vec<Value> = inp.files.iter().map(|(p, h, _)| json!([p, h])).collect();
    let mut n = Need::new(
        RULE,
        key(&[
            json!(RULE),
            json!(py::casefold(&inp.bundle_id)),
            o(&inp.text),
            o(&inp.keys),
            json!(inp.paste),
            o(&inp.menu),
            o(&inp.secret),
            o(&inp.script),
            json!(inp.drag),
            o(&inp.control),
            o(&inp.selection),
            o(&inp.file),
            Value::Array(files_key),
        ]),
        values,
    );
    n.always = true;
    n.grants = if wide {
        app_key(&inp.bundle_id)
    } else {
        String::new()
    };
    n
}
