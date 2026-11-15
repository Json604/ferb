//! ferb/orchestrator/progress.py: the live progress line (L8), built by code from the tool and its target, never
//! from model text (only a file's name, and ORCH-628's command line quoted as the task's own words).
//!
//! decision: ORCH-628

use serde_json::{Map, Value};

use ferb_sanitize::terminal::printable;

use crate::ports::Policy;
use crate::py::{prefix, rsplit_once_head, rstrip, squeeze, strip};

pub const BROWSERS: &[&str] = &["browser", "ferb_browser"];
pub const WORKING: &str = "working";
pub const AN_APP: &str = "an app";
const FILE_KINDS: &[&str] = &["write_file", "trash", "delete_permanent"];

/// progress.py:25 `TOOLS`: (with a target, without one).
pub fn tool_words(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "browser.open" | "browser.navigate" => ("opening {}", "opening a page"),
        "browser.read" | "browser.snapshot" | "browser.find" | "browser.scroll"
        | "browser.html" => ("reading {}", "reading a page"),
        "browser.screenshot" => ("looking at {}", "looking at a page"),
        "browser.wait" => ("waiting for {}", "waiting for a page"),
        "mac.snapshot" | "mac.screenshot" => ("looking at {}", "looking at an app"),
        "mac.open" => ("opening {}", "opening an app"),
        "fs.read" => ("reading {}", "reading a file"),
        "fs.list" => ("looking in {}", "looking in a folder"),
        "fs.look" => ("looking at {}", "looking at a file"),
        "fs.write" => ("writing {}", "writing a file"),
        "fs.edit" => ("editing {}", "editing a file"),
        "fs.trash" => ("moving {} to the Trash", "moving a file to the Trash"),
        "shell.run" => ("running a command", "running a command"),
        "shell.start" => ("starting a command", "starting a command"),
        "shell.wait" => ("waiting for a command", "waiting for a command"),
        "time.sleep_until" | "wait.for" => ("waiting", "waiting"),
        "owner.ask" => ("asking you", "asking you"),
        _ => return None,
    })
}

/// progress.py:47 `KINDS`: the verb of a registry kind (W2).
pub fn kind_words(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "read" => ("reading {}", "reading"),
        "navigate" => ("opening {}", "opening a page"),
        "search" => ("searching {}", "searching"),
        "open_tab" => ("opening {}", "opening"),
        "draft" => ("drafting in {}", "drafting"),
        "fill_form" => ("filling in a form in {}", "filling in a form"),
        "submit" => ("submitting in {}", "submitting"),
        "send" => ("sending from {}", "sending"),
        "post" => ("posting in {}", "posting"),
        "reply" => ("replying in {}", "replying"),
        "pay" => ("paying in {}", "paying"),
        "delete_permanent" => ("deleting {}", "deleting"),
        "trash" => ("moving {} to the Trash", "moving to the Trash"),
        "write_file" => ("writing {}", "writing a file"),
        "run_shell" => ("running a command", "running a command"),
        "applescript" => ("running a script in {}", "running a script"),
        "mac_input" => ("using {}", "using the Mac"),
        "click" => ("clicking in {}", "clicking"),
        "fill_secret" => ("filling a saved secret in {}", "filling a saved secret"),
        "mailbox_read" => ("reading mail", "reading mail"),
        "mailbox_send" => ("sending mail", "sending mail"),
        "timer" => ("setting a reminder", "setting a reminder"),
        "account_security" => (
            "changing account security in {}",
            "changing account security",
        ),
        "call" => ("starting a call in {}", "starting a call"),
        "snapshot" => ("saving a recovery point", "saving a recovery point"),
        "stop_process" => ("stopping a command", "stopping a command"),
        "owner_question" => ("asking you", "asking you"),
        _ => return None,
    })
}

/// progress.py:68 `describe`.
pub fn describe(
    policy: &dyn Policy,
    tool: &str,
    args: &Map<String, Value>,
    kind: &str,
    site: Option<&str>,
    app: Option<&str>,
) -> String {
    let (family, verb) = match tool.rfind('.') {
        Some(i) => (&tool[..i], &tool[i + 1..]),
        None => ("", tool),
    };
    let name = if BROWSERS.contains(&family) {
        format!("browser.{verb}")
    } else {
        tool.to_string()
    };
    let (with_target, alone) = tool_words(&name)
        .or_else(|| kind_words(kind))
        .unwrap_or((WORKING, WORKING));
    let target = target_of(policy, family, args, kind, site, app);
    if !target.is_empty() && with_target.contains("{}") {
        with_target.replacen("{}", &target, 1)
    } else {
        alone.to_string()
    }
}

/// progress.py:80 `WAITS_ON_COMMAND`.
pub fn waits_on_command(tool: &str) -> Option<&'static str> {
    match tool {
        "shell.wait" => Some("waiting for"),
        "shell.read" => Some("reading the output of"),
        _ => None,
    }
}

pub const COMMAND_CHARS: usize = 40;

/// progress.py:84 `waiting_for`. `tool` is one of WAITS_ON_COMMAND's (Python raises KeyError otherwise).
pub fn waiting_for(tool: &str, command: &str, minutes: i64) -> String {
    let verb = waits_on_command(tool).expect("a tool that waits on a command");
    let said = command_said(command);
    let said = if said.is_empty() {
        format!("{verb} a command")
    } else {
        format!("{verb} {said}")
    };
    if minutes >= 1 {
        format!("{said} ({minutes} min)")
    } else {
        said
    }
}

/// progress.py:93 `command_said`.
pub fn command_said(command: &str) -> String {
    let line = crate::py::splitlines(command)
        .into_iter()
        .find(|l| !strip(l).is_empty())
        .unwrap_or("");
    let mut first = squeeze(&printable(line));
    if crate::py::len(&first) > COMMAND_CHARS {
        first = format!(
            "{} …",
            rstrip(rsplit_once_head(prefix(&first, COMMAND_CHARS - 1), " "))
        );
    }
    if first.is_empty() {
        String::new()
    } else {
        format!("`{}`", first.replace('`', ""))
    }
}

/// progress.py:102 `app_name`. `installed`: (bundle id, name); `running`: (bundle id, name, pid), read only when
/// needed.
pub fn app_name(
    args: &Map<String, Value>,
    site: Option<&str>,
    installed: &[(String, String)],
    running: &dyn Fn() -> Vec<(String, String, i64)>,
) -> Option<String> {
    let mut given: Vec<String> = Vec::new();
    for v in [
        site.map(|s| Value::String(s.into())),
        args.get("app").cloned(),
        args.get("bundle_id").cloned(),
    ]
    .into_iter()
    .flatten()
    {
        if let Value::String(s) = v {
            if !strip(&s).is_empty() {
                given.push(strip(&s).to_string());
            }
        }
    }
    if given.is_empty() {
        return None;
    }
    let casefold = crate::py::casefold;
    // a dict: a later key of the same casefold replaces the earlier one's name
    let mut known: Vec<(String, String)> = Vec::new();
    for (bundle, name) in installed {
        for key in [bundle, name] {
            if !key.is_empty() && !name.is_empty() {
                let k = casefold(key);
                match known.iter_mut().find(|(kk, _)| *kk == k) {
                    Some(slot) => slot.1 = name.clone(),
                    None => known.push((k, name.clone())),
                }
            }
        }
    }
    for value in &given {
        if let Some((_, name)) = known.iter().find(|(k, _)| *k == casefold(value)) {
            return Some(name.clone());
        }
    }
    for (bundle, name, pid) in running() {
        if !name.is_empty()
            && given
                .iter()
                .any(|v| casefold(v) == casefold(&bundle) || *v == format!("pid:{pid}"))
        {
            return Some(name);
        }
    }
    Some(AN_APP.to_string())
}

/// `PurePosixPath(p).name`.
fn posix_name(path: &str) -> String {
    path.split('/')
        .rfind(|p| !p.is_empty() && *p != ".")
        .unwrap_or("")
        .to_string()
}

fn target_of(
    policy: &dyn Policy,
    family: &str,
    args: &Map<String, Value>,
    kind: &str,
    site: Option<&str>,
    app: Option<&str>,
) -> String {
    let file = match args.get("path") {
        Some(Value::String(path)) if !strip(path).is_empty() => {
            let trimmed = path.trim_end_matches('/');
            let name = posix_name(if trimmed.is_empty() { "/" } else { trimmed });
            plain(if name.is_empty() { path } else { &name })
        }
        _ => String::new(),
    };
    if !file.is_empty() && (FILE_KINDS.contains(&kind) || family == "fs") {
        return file;
    }
    if BROWSERS.contains(&family) {
        return match site {
            Some(s) if !s.is_empty() => plain(s),
            _ => plain(&host(policy, args.get("url"))),
        };
    }
    if family == "mac" {
        return match app {
            Some(a) if !a.is_empty() => plain(a),
            _ => {
                let named = [args.get("app"), args.get("bundle_id")]
                    .into_iter()
                    .any(|v| v.is_some_and(ferb_py::value::truthy));
                if named {
                    AN_APP.to_string()
                } else {
                    String::new()
                }
            }
        };
    }
    if !file.is_empty() {
        file
    } else {
        plain(&host(policy, args.get("url")))
    }
}

fn host(policy: &dyn Policy, url: Option<&Value>) -> String {
    match url {
        Some(Value::String(u)) if !u.is_empty() => policy.url_host(u).unwrap_or_default(),
        _ => String::new(),
    }
}

/// progress.py:140 `_plain`: one line, no control characters.
fn plain(value: &str) -> String {
    squeeze(&printable(value))
}
