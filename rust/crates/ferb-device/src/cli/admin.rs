//! The developer toolbox's RPC clients (ferb/cli/admin.py): `verdict`, `ps`, `browsers [pair|unpair]`, `restore`
//! and `memory`, each printing what the daemon answered as Python did. `trace` and `stats` need the traces part and
//! the playbooks (not ported yet); `shown_bytes` and `status_label` are shared with setup.

use std::io::Write;

use serde_json::{json, Map, Value};

use super::{CliError, Rpc};

/// admin.py:20: done_unverified reads "not confirmed" (V5).
pub fn status_label(status: &str) -> String {
    if status == "done_unverified" {
        "not confirmed".into()
    } else {
        status.into()
    }
}

/// admin.py:507: a size as Finder shows it (decimal units).
pub fn shown_bytes(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.0} KB", n / 1e3)
    }
}

fn say(out: &mut dyn Write, line: &str) {
    let _ = writeln!(out, "{line}");
}

fn get(v: &Value, key: &str) -> String {
    v.get(key)
        .map(ferb_py::value::py_str)
        .unwrap_or_else(|| "None".into())
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// admin.py:109.
pub fn verdict(
    client: &mut dyn Rpc,
    task_id: &str,
    verdict: &str,
    note: &str,
    out: &mut dyn Write,
) -> Result<i32, CliError> {
    let r = client.call(
        "verdict.set",
        Some(json!({"task_id": task_id, "verdict": verdict, "note": note})),
    )?;
    say(
        out,
        &format!(
            "Recorded {} for {}.",
            get(&r, "verdict"),
            get(&r, "task_id")
        ),
    );
    Ok(0)
}

/// admin.py:213.
pub fn ps(client: &mut dyn Rpc, out: &mut dyn Write) -> Result<i32, CliError> {
    let procs = client.call("ps.list", None)?["processes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if procs.is_empty() {
        say(out, "No processes.");
    }
    for p in procs {
        let state = if truthy(p.get("running")) {
            "running".to_string()
        } else {
            format!("exited {}", get(&p, "exit_code"))
        };
        let bg = if truthy(p.get("long_lived")) {
            ", background"
        } else {
            ""
        };
        let command: String = get(&p, "command").chars().take(80).collect();
        say(
            out,
            &format!(
                "{}  {state}{bg}  task {}  {command}",
                get(&p, "id"),
                get(&p, "owner_task")
            ),
        );
    }
    Ok(0)
}

pub const INSTALL_WAIT_S: f64 = 300.0;

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// admin.py:139: the extension isn't in that profile; the folder goes on the clipboard (`copy`, false when it couldn't
/// run) and FERB waits until the browser has it.
fn guide_install(
    client: &mut dyn Rpc,
    r: &Value,
    profile: Option<&str>,
    browser: Option<&str>,
    out: &mut dyn Write,
    sleep: &mut dyn FnMut(f64),
    copy: &mut dyn FnMut(&str) -> bool,
) -> Result<bool, CliError> {
    let path = get(r, "extension_dir");
    let copied = if copy(&path) {
        " (copied to your clipboard)"
    } else {
        ""
    };
    say(
        out,
        &format!(
            "FERB's extension isn't installed in {} “{}” yet. Its extensions page is open there:",
            capitalize(&get(r, "browser")),
            get(r, "name")
        ),
    );
    say(
        out,
        "  (If it didn't open: in that profile's window, ⋮ menu → Extensions → Manage Extensions.)",
    );
    say(out, "  1. Turn on Developer mode (top right).");
    say(out, "  2. Click “Load unpacked”.");
    say(
        out,
        &format!("  3. Press ⌘⇧G, paste the folder{copied}, press Return, then click Select:"),
    );
    say(out, &format!("     {path}"));
    say(out, "Waiting for it…");
    let _ = out.flush();
    for _ in 0..(INSTALL_WAIT_S / 2.0) as usize {
        let r = client.call(
            "browsers.installed",
            Some(json!({"profile": profile, "browser": browser})),
        )?;
        if truthy(r.get("installed")) {
            return Ok(true);
        }
        sleep(2.0);
    }
    say(
        out,
        "Didn't see it installed after 5 minutes. Run this again when it's loaded.",
    );
    Ok(false)
}

/// admin.py:166: `ferb browsers [pair|unpair <profile>]`.
#[allow(clippy::too_many_arguments)]
pub fn browsers(
    client: &mut dyn Rpc,
    cmd: Option<&str>,
    profile: Option<&str>,
    browser: Option<&str>,
    out: &mut dyn Write,
    sleep: &mut dyn FnMut(f64),
    copy: &mut dyn FnMut(&str) -> bool,
) -> Result<i32, CliError> {
    let named = json!({"profile": profile, "browser": browser});
    if cmd == Some("pair") {
        say(
            out,
            &format!(
                "Opening the FERB pairing page in {}…",
                profile.unwrap_or("None")
            ),
        );
        let mut r = client.call("browsers.pair", Some(named.clone()))?;
        if truthy(r.get("needs_install")) {
            if !guide_install(client, &r, profile, browser, out, sleep, copy)? {
                return Ok(1);
            }
            say(out, "Installed. Now pairing…");
            r = client.call("browsers.pair", Some(named))?;
        }
        if truthy(r.get("paired")) {
            say(
                out,
                &format!(
                    "Paired {} profile “{}” ({}).",
                    get(&r, "browser"),
                    get(&r, "name"),
                    get(&r, "folder")
                ),
            );
            return Ok(0);
        }
        let hint = r
            .get("hint")
            .map(ferb_py::value::py_str)
            .unwrap_or_else(|| "no answer from the extension".into());
        say(out, &format!("Not paired: {hint}"));
        return Ok(1);
    }
    if cmd == Some("unpair") {
        let r = client.call("browsers.unpair", Some(named))?;
        say(
            out,
            if truthy(r.get("unpaired")) {
                "Unpaired."
            } else {
                "That profile wasn't paired."
            },
        );
        return Ok(0);
    }
    let r = client.call("browsers.list", None)?;
    for p in r["profiles"].as_array().cloned().unwrap_or_default() {
        let connected = truthy(p.get("connected"));
        let version = p
            .get("version")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str);
        let mut state = if truthy(p.get("ferb_identity")) {
            if !truthy(p.get("paired")) {
                "FERB's own profile, not paired yet".to_string()
            } else if connected {
                format!(
                    "FERB's own profile, connected{}",
                    version
                        .as_ref()
                        .map(|v| format!(", extension {v}"))
                        .unwrap_or_default()
                )
            } else {
                "FERB's own profile, paired; not open right now (FERB opens it when a task needs it)".to_string()
            }
        } else if !truthy(p.get("paired")) {
            "not paired".to_string()
        } else if truthy(p.get("refused")) {
            format!("refused: {}", get(&p, "refused"))
        } else {
            format!(
                "paired, {}{}",
                if connected {
                    "connected"
                } else {
                    "not connected"
                },
                match (&version, connected) {
                    (Some(v), true) => format!(", extension {v}"),
                    _ => String::new(),
                }
            )
        };
        if !connected {
            let last = p
                .get("last_version")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .or(version.clone());
            if let Some(last) = last {
                state += &format!(", last connected version {last}");
            }
        }
        let email = if truthy(p.get("email")) {
            format!(" <{}>", get(&p, "email"))
        } else {
            String::new()
        };
        say(
            out,
            &format!(
                "{:<7} {}{email} ({}): {state}",
                get(&p, "browser"),
                get(&p, "name"),
                get(&p, "folder")
            ),
        );
    }
    if let Some(unpaired) = r.get("unpaired_connections").and_then(Value::as_object) {
        for (browser, count) in unpaired {
            say(out, &format!(
                "{browser}: {} live extension connection(s) have no pairing (a reinstalled extension gets a new identity). Pair it in FERB's Settings → Browsers, or run `ferb browsers pair <profile> --browser {browser}`.",
                ferb_py::value::py_str(count)
            ));
        }
    }
    Ok(0)
}

/// ferb/memory/items.py LABELS.
pub fn origin_label(origin: &str) -> Option<&'static str> {
    match origin {
        "owner_stated" => Some("your words"),
        "inferred" => Some("FERB's understanding"),
        "observed" => Some("observed"),
        "third_party" => Some("from outside content"),
        _ => None,
    }
}

/// admin.py:228: the last day an item held (the day before the stamp's midnight, in its own offset).
fn last_day(stamp: &str) -> Result<String, CliError> {
    let t = ferb_py::time::DateTime::from_isoformat(stamp).map_err(|_| CliError::Ferb {
        kind: "ValueError".into(),
        message: format!(
            "Invalid isoformat string: {}",
            crate::py::py_repr_str(stamp)
        ),
    })?;
    let d = t.plus(0, -1).map_err(|_| CliError::Ferb {
        kind: "OverflowError".into(),
        message: "date value out of range".into(),
    })?;
    Ok(format!("{:04}-{:02}-{:02}", d.year, d.month, d.day))
}

/// admin.py:234.
fn item_line(i: &Value) -> Result<String, CliError> {
    let mut extra = String::new();
    if get(i, "status") == "proposed" && get(i, "origin") == "observed" {
        extra = format!(" — seen on {} of 3 days", get(i, "evidence_days"));
    }
    if truthy(i.get("in_profile")) {
        extra += " — in the core profile";
    }
    let until = if truthy(i.get("valid_to")) {
        format!(" (until {})", last_day(&get(i, "valid_to"))?)
    } else {
        String::new()
    };
    let origin = get(i, "origin");
    let label = origin_label(&origin).ok_or_else(|| CliError::Ferb {
        kind: "KeyError".into(),
        message: crate::py::py_repr_str(&origin),
    })?;
    Ok(format!(
        "  {}  [{} · {} · {}] {}  ({label}, {}{until}){extra}",
        get(i, "id"),
        get(i, "kind"),
        get(i, "subject"),
        get(i, "key"),
        get(i, "content"),
        get(i, "status")
    ))
}

fn pretty(v: &Value) -> String {
    ferb_py::json::dumps_with(
        v,
        ferb_py::json::Style {
            ascii: false,
            sort_keys: false,
            indent: Some(2),
        },
    )
}

/// `ferb memory`'s arguments (the parser's `memory` subcommand).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryArgs {
    pub mcmd: String,
    pub subject: Option<String>,
    pub content: Option<String>,
    pub kind: Option<String>,
    pub key: Option<String>,
    pub id: Option<String>,
    pub query: Option<String>,
    pub app: Option<String>,
    pub person: Option<String>,
    pub batch: Option<String>,
    pub all: bool,
    pub proposed: bool,
    pub yes: bool,
    pub keep_sources: bool,
}

/// admin.py:245: `ferb memory ...`.
pub fn memory(
    client: &mut dyn Rpc,
    args: &MemoryArgs,
    ask: &mut dyn FnMut(&str) -> String,
    out: &mut dyn Write,
) -> Result<i32, CliError> {
    let c = args.mcmd.as_str();
    if c == "embeddings" || c == "eval-export" {
        let op = if c == "embeddings" {
            "embedding"
        } else {
            "eval_export"
        };
        client.set_timeout(None);
        say(
            out,
            &pretty(&client.call("memory.learning", Some(json!({"op": op})))?),
        );
        return Ok(0);
    }
    if c == "profile" {
        say(
            out,
            &get(&client.call("memory.profile", Some(json!({})))?, "markdown"),
        );
        return Ok(0);
    }
    let id = args.id.clone().unwrap_or_default();
    if matches!(c, "digest" | "search" | "rollback") {
        let mut params = Map::new();
        params.insert("op".into(), json!(c));
        for (k, v) in [
            ("query", &args.query),
            ("kind", &args.kind),
            ("app", &args.app),
            ("person", &args.person),
            ("key", &args.key),
            ("batch", &args.batch),
        ] {
            if let Some(v) = v {
                params.insert(k.into(), json!(v));
            }
        }
        if c == "search" || c == "rollback" || c == "digest" {
            // `text` is no argument of these subcommands; Python reads it only if present
        }
        say(
            out,
            &pretty(&client.call("memory.learning", Some(Value::Object(params)))?),
        );
    } else if (c == "edit" || c == "forget") && id.starts_with("m-") {
        let mut params = Map::new();
        params.insert("op".into(), json!(c));
        params.insert("id".into(), json!(id));
        if c == "edit" {
            params.insert("text".into(), json!(args.content));
        }
        if c == "forget" {
            let plan = client.call("memory.learning", Some(Value::Object(params.clone())))?;
            say(out, &pretty(&plan));
            let answer = if args.yes {
                "y".to_string()
            } else {
                ask("Permanently forget these memories? [y/N] ")
            };
            if !matches!(
                crate::py::strip(&answer).to_lowercase().as_str(),
                "y" | "yes"
            ) {
                return Ok(1);
            }
            params.insert("confirm".into(), json!(true));
        }
        say(
            out,
            &pretty(&client.call("memory.learning", Some(Value::Object(params)))?),
        );
    } else if c == "list" {
        let status = if args.all {
            "all"
        } else if args.proposed {
            "proposed"
        } else {
            "active"
        };
        let items = client.call("memory.list", Some(json!({"status": status})))?["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let lines: Result<Vec<String>, CliError> = items.iter().map(item_line).collect();
        let text = lines?.join("\n");
        say(
            out,
            if text.is_empty() {
                "  (nothing)"
            } else {
                &text
            },
        );
    } else if c == "show" {
        let r = client.call("memory.show", Some(json!({"subject": args.subject})))?;
        say(out, &format!("{}:", get(&r, "subject")));
        for i in r["items"].as_array().cloned().unwrap_or_default() {
            say(out, &item_line(&i)?);
            let sources: Vec<String> = i["sources"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(ferb_py::value::py_str)
                .collect();
            say(
                out,
                &format!(
                    "      sources: {}",
                    if sources.is_empty() {
                        "-".to_string()
                    } else {
                        sources.join(", ")
                    }
                ),
            );
        }
    } else if c == "add" {
        let i = client.call(
            "memory.add",
            Some(json!({"content": args.content, "kind": args.kind, "subject": args.subject, "key": args.key})),
        )?;
        say(out, &format!("Remembered:\n{}", item_line(&i)?));
    } else if c == "edit" {
        let i = client.call(
            "memory.edit",
            Some(json!({"id": id, "content": args.content})),
        )?;
        say(
            out,
            &format!(
                "Updated (the old version stays as history):\n{}",
                item_line(&i)?
            ),
        );
    } else if c == "retract" {
        client.call("memory.retract", Some(json!({"id": id})))?;
        say(
            out,
            &format!("Retracted {id}: kept as history, never used."),
        );
    } else if c == "forget" {
        let p = json!({"id": id, "keep_sources": args.keep_sources});
        let planned = client.call("memory.forget", Some(p.clone()))?;
        let will: Vec<String> = planned["will_delete"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|x| format!("  - {}", ferb_py::value::py_str(x)))
            .collect();
        let keeps = if truthy(planned.get("keeps")) {
            format!("\n{}", get(&planned, "keeps"))
        } else {
            String::new()
        };
        say(
            out,
            &format!("This permanently deletes:\n{}{keeps}", will.join("\n")),
        );
        let answer = if args.yes {
            "y".to_string()
        } else {
            ask("Forget it? [y/N] ")
        };
        if !matches!(
            crate::py::strip(&answer).to_lowercase().as_str(),
            "y" | "yes"
        ) {
            say(out, "Nothing deleted.");
            return Ok(1);
        }
        let mut confirmed = p;
        confirmed["confirm"] = json!(true);
        client.call("memory.forget", Some(confirmed))?;
        say(out, "Forgotten.");
    }
    Ok(0)
}

/// admin.py:307: what each recorded change is called.
pub fn reason_words(reason: &str) -> Option<&'static str> {
    match reason {
        "overwrite" => Some("overwrote"),
        "edit" => Some("edited"),
        "trash" | "trash-unverified" => Some("moved to Trash"),
        "trash copy" => Some("Trash copy"),
        _ => None,
    }
}

/// admin.py ANSI: terminal control sequences and C0 controls (not tab, newline, vertical tab, form feed, CR).
pub fn strip_ansi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\x1b' {
            let next = chars.get(i + 1).copied();
            match next {
                Some('[') => {
                    let mut j = i + 2;
                    while j < chars.len()
                        && (chars[j].is_ascii_digit() || ";?<>=".contains(chars[j]))
                    {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_alphabetic() {
                        i = j + 1;
                        continue;
                    }
                }
                Some(']') => {
                    if let Some(end) = chars[i + 2..].iter().position(|&x| x == '\x07') {
                        i = i + 2 + end + 1;
                        continue;
                    }
                }
                Some('(') | Some(')') => {
                    if chars
                        .get(i + 2)
                        .is_some_and(|x| x.is_ascii_uppercase() || x.is_ascii_digit())
                    {
                        i += 3;
                        continue;
                    }
                }
                Some('7') | Some('8') | Some('=') | Some('>') => {
                    i += 2;
                    continue;
                }
                _ => {}
            }
            // ESC itself is one of [\x0e-\x1f]
            i += 1;
            continue;
        }
        if matches!(c, '\x00'..='\x08' | '\x0e'..='\x1f') {
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Python's `str.isprintable()` for one character, for the characters file names hold: no control, separator other
/// than the space, or format character (an approximation of Unicode's Cc, Cf, Zs, Zl, Zp, Co classes).
pub fn printable_char(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(c,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}'
        | '\u{fff9}'..='\u{fffb}' | '\u{e000}'..='\u{f8ff}')
}

/// admin.py:311: a file name as plain text.
pub fn shown(path: &str) -> String {
    strip_ansi(path)
        .chars()
        .map(|c| if printable_char(c) { c } else { '?' })
        .collect()
}

/// admin.py:316: `ferb restore [id]`.
pub fn restore(
    client: &mut dyn Rpc,
    rid: Option<&str>,
    out: &mut dyn Write,
) -> Result<i32, CliError> {
    if let Some(rid) = rid.filter(|r| !r.is_empty()) {
        let r = client.call("recovery.restore", Some(json!({"id": rid})))?;
        let (was, now) = (shown(&get(&r, "original")), shown(&get(&r, "restored_to")));
        let in_place = truthy(r.get("in_place"));
        let text = if get(&r, "kind") == "trash" {
            if in_place {
                format!("Put {was} back from the Trash.")
            } else {
                format!("Something is at {was} now, so the file from the Trash was put back beside it as {now}.")
            }
        } else if in_place {
            format!("Restored {was} to what it was before FERB changed it.")
        } else {
            format!("{was} has changed since FERB changed it, so it was left as it is. The version from before FERB's change is now at {now}.")
        };
        say(out, &text);
        return Ok(0);
    }
    let r = client.call("recovery.list", None)?;
    let rows = r["records"].as_array().cloned().unwrap_or_default();
    let days = get(&r, "days");
    let total = r
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(rows.len() as u64);
    if rows.is_empty() {
        say(
            out,
            "Nothing to put back: FERB hasn't changed or deleted any files lately.",
        );
        return Ok(0);
    }
    say(out, &format!(
        "What FERB can put back, newest first (copies of changed files are kept {days} days; items it moved to the Trash, while they are still there):"
    ));
    if total > rows.len() as u64 {
        say(
            out,
            &format!("Showing the newest {} of {total}.", rows.len()),
        );
    }
    for row in &rows {
        let at_text = get(row, "at");
        let at =
            super::setup::local_time(&at_text, "%a %d %b %H:%M").ok_or_else(|| CliError::Ferb {
                kind: "ValueError".into(),
                message: format!(
                    "Invalid isoformat string: {}",
                    crate::py::py_repr_str(&at_text)
                ),
            })?;
        let id = get(row, "id");
        let shown_id: String = if id.chars().count() > 12 {
            id.chars().take(8).collect()
        } else {
            id
        };
        if get(row, "kind") == "snapshot" {
            let saved = row
                .get("saved_to")
                .filter(|v| ferb_py::value::truthy(v))
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            say(out, &format!(
                "  {shown_id}  {at}  {:<14}  {}: a recovery point of the whole disk you can open from macOS Recovery or Time Machine (admin rights)",
                "disk snapshot",
                shown(&saved)
            ));
            continue;
        }
        let reason = row
            .get("reason")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str);
        let what = match &reason {
            Some(r) => reason_words(r)
                .map(String::from)
                .unwrap_or_else(|| r.clone()),
            None => get(row, "kind"),
        };
        let mut gone = if truthy(row.get("available")) {
            String::new()
        } else if get(row, "kind") == "trash" {
            "  (not found in the Trash)".into()
        } else {
            "  (its copy is gone)".into()
        };
        if truthy(row.get("needs_owner")) {
            gone = "  (it changed while FERB moved it, so FERB won't put it back: look at it in the Trash and move it yourself if it's yours)".into();
        }
        say(
            out,
            &format!(
                "  {shown_id}  {at}  {what:<14}  {}{gone}",
                shown(&get(row, "original"))
            ),
        );
    }
    say(out, "Put one back: ferb restore <id>");
    Ok(0)
}
