//! `ferb <command>` (ferb/cli/__init__.py `main`): the parsed command line dispatched to the setup commands and the
//! toolbox, with Python's exit statuses: 0 done, 1 failed (one plain line on stderr), 2 a command line argparse
//! refuses or a toolbox command with no daemon, 3 the service switched off in Login Items, 130 a stop.
//!
//! What the program composes from other parts (the setup env's seams, the socket) comes from a [`Host`]. Commands
//! whose code belongs to parts not ported yet (`trace`, `stats`, `runtimes`, `memory dashboard`, `browsers
//! extension`, `mcp`, `daemon run`) answer exit 1 with a line saying so.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;

use super::admin::{self, MemoryArgs};
use super::args::{parse, Parsed, LOG_FLAGS};
use super::setup::{self, Env};
use super::{logs, plain_error, CliError, Rpc};

/// ferb/trace/lines.py `step_line`.
pub type StepLine = Box<dyn Fn(&Value) -> String>;

/// What `run` needs from the program around it.
pub trait Host {
    fn version(&self) -> String;
    fn out(&mut self) -> &mut dyn Write;
    fn err(&mut self) -> &mut dyn Write;
    /// FERB_OWN_SESSION=1: a session of its own (DMN-537).
    fn own_session(&mut self);
    /// ferb/store/salvage.py: refuse a newer FERB's database, then the lines about files set aside.
    fn salvage(&mut self) -> Result<Vec<String>, CliError>;
    /// The setup commands' env (`every_browser`: uninstall's, which lists every known browser folder).
    fn env(&mut self, every_browser: bool) -> Result<Env<'_>, CliError>;
    /// A connection to the daemon as the `cli` role.
    fn connect(&mut self) -> Result<Box<dyn Rpc>, CliError>;
    fn ask(&mut self, prompt: &str) -> String;
    fn sleep(&mut self, seconds: f64);
    /// Puts text on the clipboard (`pbcopy`); false when it couldn't run.
    fn copy(&mut self, text: &str) -> bool;
    /// ferb/trace/lines.py `step_line` for a `tool.finished` event; None until the traces part is ported.
    fn step_line(&self) -> Option<StepLine>;
}

fn s(ns: &std::collections::BTreeMap<String, Value>, k: &str) -> Option<String> {
    ns.get(k).and_then(Value::as_str).map(String::from)
}

fn b(ns: &std::collections::BTreeMap<String, Value>, k: &str) -> bool {
    ns.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn not_ported(host: &mut dyn Host, what: &str) -> i32 {
    let _ = writeln!(
        host.err(),
        "ferb: `ferb {what}` isn't part of the Rust program yet; the Python build has it"
    );
    1
}

/// __init__.py:170.
pub fn run(argv: &[String], host: &mut dyn Host) -> i32 {
    let ns = match parse(argv) {
        Parsed::Version => {
            let v = host.version();
            let _ = writeln!(host.out(), "ferb {v}");
            return 0;
        }
        Parsed::Error(e) => {
            let _ = writeln!(host.err(), "ferb: error: {e}");
            return 2;
        }
        Parsed::Args(ns) => ns,
    };
    let cmd = s(&ns, "cmd").unwrap_or_default();
    match cmd.as_str() {
        "daemon" | "stats" | "mcp" | "runtimes" | "trace" => return not_ported(host, &cmd),
        "browsers" if s(&ns, "bcmd").as_deref() == Some("extension") => {
            return not_ported(host, "browsers extension")
        }
        "memory" if s(&ns, "mcmd").as_deref() == Some("dashboard") => {
            return not_ported(host, "memory dashboard")
        }
        _ => {}
    }
    if matches!(cmd.as_str(), "setup" | "doctor" | "uninstall") {
        if cmd == "setup" {
            match host.salvage() {
                Ok(lines) => {
                    for line in lines {
                        let _ = writeln!(host.out(), "{line}");
                    }
                }
                Err(e) => {
                    let _ = writeln!(host.err(), "{}", plain_error(&e));
                    return 1;
                }
            }
            if b(&ns, "service") || b(&ns, "cleanup") {
                host.own_session();
            }
        }
        let outcome = (|| -> Result<i32, CliError> {
            let mut env = host.env(cmd == "uninstall")?;
            if cmd == "setup" {
                if b(&ns, "cleanup") {
                    return match s(&ns, "ok") {
                        Some(ok) if !ok.is_empty() => setup::cleanup_apply(&mut env, &ok),
                        _ => setup::cleanup_preview(&mut env),
                    };
                }
                if b(&ns, "service") {
                    env.inst.app_service = Some(b(&ns, "app_service"));
                    return setup::service_setup(&mut env, b(&ns, "update"));
                }
                if b(&ns, "codex_logout") {
                    return setup::codex_logout(&mut env);
                }
                if b(&ns, "cli") {
                    return setup::cli_setup(&mut env);
                }
                if b(&ns, "codex_login") {
                    return setup::codex_login(&mut env);
                }
                if b(&ns, "claude_logout") {
                    return setup::claude_logout(&mut env);
                }
                return setup::setup(&mut env, b(&ns, "update"));
            }
            if cmd == "doctor" {
                return setup::doctor(&mut env);
            }
            setup::uninstall(&mut env, b(&ns, "purge"), b(&ns, "yes"))
        })();
        return match outcome {
            Ok(rc) => rc,
            Err(CliError::Interrupted) => 130,
            Err(e) => {
                let _ = writeln!(host.err(), "{}", plain_error(&e));
                1
            }
        };
    }
    let outcome = (|| -> Result<i32, CliError> {
        let mut client = host.connect()?;
        let rc = toolbox(&cmd, &ns, client.as_mut(), host);
        client.close();
        rc
    })();
    match outcome {
        Ok(rc) => rc,
        Err(e @ CliError::NotRunning(_)) => {
            let _ = writeln!(host.err(), "{}", plain_error(&e));
            2
        }
        Err(CliError::Interrupted) => 130,
        Err(e) => {
            let _ = writeln!(host.err(), "{}", plain_error(&e));
            1
        }
    }
}

/// The host's output as a writer, through the cell the toolbox's other closures share.
struct HostOut<'a, 'b>(&'a RefCell<&'b mut dyn Host>);

impl Write for HostOut<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().out().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().out().flush()
    }
}

fn toolbox(
    cmd: &str,
    ns: &std::collections::BTreeMap<String, Value>,
    client: &mut dyn Rpc,
    host: &mut dyn Host,
) -> Result<i32, CliError> {
    if cmd == "logs" {
        let filters: Vec<String> = LOG_FLAGS
            .iter()
            .filter(|f| b(ns, f))
            .map(|f| f.to_string())
            .collect();
        let step = host.step_line();
        let missing = |_: &Value| "(a tool step: its line comes with the traces port)".to_string();
        let line: &dyn Fn(&Value) -> String = match &step {
            Some(f) => f.as_ref(),
            None => &missing,
        };
        return logs::run(client, &filters, host.out(), None, line);
    }
    let cell = RefCell::new(host);
    let mut out = HostOut(&cell);
    match cmd {
        "verdict" => admin::verdict(
            client,
            &s(ns, "task_id").unwrap_or_default(),
            &s(ns, "verdict").unwrap_or_default(),
            &s(ns, "note").unwrap_or_default(),
            &mut out,
        ),
        "memory" => {
            let m = MemoryArgs {
                mcmd: s(ns, "mcmd").unwrap_or_default(),
                subject: s(ns, "subject"),
                content: s(ns, "content"),
                kind: s(ns, "kind"),
                key: s(ns, "key"),
                id: s(ns, "id"),
                query: s(ns, "query"),
                app: s(ns, "app"),
                person: s(ns, "person"),
                batch: s(ns, "batch"),
                all: b(ns, "all"),
                proposed: b(ns, "proposed"),
                yes: b(ns, "yes"),
                keep_sources: b(ns, "keep_sources"),
            };
            let mut ask = |prompt: &str| cell.borrow_mut().ask(prompt);
            admin::memory(client, &m, &mut ask, &mut out)
        }
        "restore" => admin::restore(client, s(ns, "id").as_deref(), &mut out),
        "ps" => admin::ps(client, &mut out),
        "browsers" => {
            let (bcmd, profile, browser) = (s(ns, "bcmd"), s(ns, "profile"), s(ns, "browser"));
            let mut sleep = |secs: f64| cell.borrow_mut().sleep(secs);
            let mut copy = |t: &str| cell.borrow_mut().copy(t);
            admin::browsers(
                client,
                bcmd.as_deref(),
                profile.as_deref(),
                browser.as_deref(),
                &mut out,
                &mut sleep,
                &mut copy,
            )
        }
        _ => Ok(2),
    }
}

/// The program's home for the setup commands (`$FERB_HOME` or ~/.ferb) and its socket.
pub fn socket_path(home: &std::path::Path) -> PathBuf {
    home.join("run").join("ferb.sock")
}
