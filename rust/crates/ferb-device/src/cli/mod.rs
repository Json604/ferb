//! The `ferb` program's internal commands and developer toolbox (ferb/cli): the argument parser, the socket client,
//! the setup commands FERB.app runs, and the toolbox's pure RPC clients. Errors are one plain line
//! ([`plain_error`]), never a trace.
//!
//! Not ported here (each needs a part that isn't ported yet): `ferb trace` (ferb/trace render and timing), `ferb
//! stats` (ferb/trace lines, ferb/memory playbooks), `ferb runtimes` (ferb/runtimes catalog), `ferb logs`'s
//! `tool.finished` lines (ferb/trace lines), `ferb memory dashboard` (ferb/memory dashboard), `ferb browsers extension
//! package|install` (ferb/act/browser autoinstall), `ferb mcp` (ported in ferb-runtimes) and `ferb daemon run` (ferbd).

pub mod admin;
pub mod args;
#[cfg(unix)]
pub mod client;
pub mod logs;
pub mod run;
pub mod setup;
pub mod tokenizer;

use serde_json::Value;

use crate::install::InstallError;

/// What FERB.app runs (ui/mac/FERBNotch: AppSetup.swift, SetupModel.swift, SettingsPages.swift); they keep working.
pub const APP_COMMANDS: [&[&str]; 12] = [
    &["setup", "--service"],
    &["setup", "--service", "--update"],
    &["setup", "--service", "--app-service"],
    &["setup", "--service", "--update", "--app-service"],
    &["setup", "--codex-login"],
    &["setup", "--codex-logout"],
    &["setup", "--claude-logout"],
    &["setup", "--cli"],
    &["setup", "--cleanup"],
    &["uninstall"],
    &["uninstall", "--purge", "--yes"],
    &["mcp"],
];

/// `ferb/cli/__init__.py` OWN_SESSION_ENV (DMN-537).
pub const OWN_SESSION_ENV: &str = "FERB_OWN_SESSION";
/// ferb/daemon/server.py INTERNAL: the reply named only the exception's type.
pub const INTERNAL: i64 = ferb_rpc::wire::INTERNAL;

/// What a command failed with, as the Python exception it stands for.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// ferb.core.errors.NotRunning.
    #[error("{0}")]
    NotRunning(String),
    /// ferb.cli.client.RpcError.
    #[error("{message}")]
    Rpc { code: i64, message: String },
    /// Another FerbError (`kind` is its class: Unsupported, InsecureConfig, ...).
    #[error("{message}")]
    Ferb { kind: String, message: String },
    #[error(transparent)]
    Install(#[from] InstallError),
    /// An OSError.
    #[error("{0}")]
    Io(String),
    /// KeyboardInterrupt: a stop (FERB.app's SIGTERM, Ctrl-C) held until the service was loaded again.
    #[error("")]
    Interrupted,
}

impl CliError {
    pub fn type_name(&self) -> &str {
        match self {
            CliError::NotRunning(_) => "NotRunning",
            CliError::Rpc { .. } => "RpcError",
            CliError::Ferb { kind, .. } => kind,
            CliError::Install(e) => e.type_name(),
            CliError::Io(_) => "OSError",
            CliError::Interrupted => "KeyboardInterrupt",
        }
    }

    pub fn ferb(message: impl Into<String>) -> Self {
        CliError::Ferb {
            kind: "FerbError".into(),
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Io(crate::py::os_error_text(&e, None))
    }
}

impl From<ferb_core::config::ConfigError> for CliError {
    fn from(e: ferb_core::config::ConfigError) -> Self {
        match e {
            ferb_core::config::ConfigError::Insecure(m) => CliError::Ferb {
                kind: "InsecureConfig".into(),
                message: m,
            },
            other => CliError::Ferb {
                kind: "ConfigError".into(),
                message: other.to_string(),
            },
        }
    }
}

/// __init__.py:27: one line for an error, plain cause first.
pub fn plain_error(e: &CliError) -> String {
    if let CliError::Rpc { code, message } = e {
        if *code == INTERNAL {
            return format!("ferb: that failed inside FERB ({message}); `ferb stats` lists it");
        }
    }
    let text = e.to_string();
    let text = crate::py::strip(&text);
    if text.is_empty() {
        format!("ferb: {}", e.type_name())
    } else {
        format!("ferb: {text}")
    }
}

/// A connection to the daemon (ferb/cli/client.py `Client`): `call` sends the method with its params (None: `{}`).
pub trait Rpc {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, CliError>;
    fn close(&mut self) {}
    /// For calls known to take long; None waits forever.
    fn set_timeout(&mut self, _seconds: Option<f64>) {}
    /// Notifications until the daemon ends the stream; `on_event` returns false to stop.
    fn stream(
        &mut self,
        _method: &str,
        _params: Option<Value>,
        _on_event: &mut dyn FnMut(Value) -> bool,
    ) -> Result<(), CliError> {
        Err(CliError::ferb("this connection doesn't stream"))
    }
}

/// Opens a connection (a NotRunning error when no daemon answers).
pub type Connect<'a> = Box<dyn FnMut() -> Result<Box<dyn Rpc>, CliError> + 'a>;

/// __init__.py:39 (DMN-537): with FERB_OWN_SESSION=1 the command moves to a session of its own.
#[cfg(unix)]
pub fn own_session(env_value: Option<&str>) {
    if env_value != Some("1") {
        return;
    }
    // SAFETY: setsid takes no arguments; a refusal (already a group leader) is ignored, as in Python.
    unsafe {
        libc::setsid();
    }
}

/// setup.py:150 `_finish_install`: a stop (SIGTERM, SIGINT) while the service is being (re)installed is held until
/// it is loaded again; `caught` tells work in progress to end early.
pub trait Signals {
    fn hold(&mut self);
    fn caught(&self) -> bool;
    /// Restores the previous handlers; true when a stop arrived meanwhile.
    fn release(&mut self) -> bool;
}

/// No signals held (tests, and a command not on the main thread).
#[derive(Default)]
pub struct NoSignals;

impl Signals for NoSignals {
    fn hold(&mut self) {}
    fn caught(&self) -> bool {
        false
    }
    fn release(&mut self) -> bool {
        false
    }
}

#[cfg(unix)]
pub mod signals {
    //! The real holder: SIGTERM and SIGINT counted, not acted on, until released.
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CAUGHT: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn count(_sig: libc::c_int) {
        CAUGHT.fetch_add(1, Ordering::SeqCst);
    }

    #[derive(Default)]
    pub struct Held {
        previous: Vec<(libc::c_int, libc::sighandler_t)>,
    }

    impl super::Signals for Held {
        fn hold(&mut self) {
            CAUGHT.store(0, Ordering::SeqCst);
            for sig in [libc::SIGTERM, libc::SIGINT] {
                // SAFETY: `count` is async-signal-safe (one atomic add); the previous handler is kept to restore.
                let prev = unsafe { libc::signal(sig, count as *const () as libc::sighandler_t) };
                self.previous.push((sig, prev));
            }
        }
        fn caught(&self) -> bool {
            CAUGHT.load(Ordering::SeqCst) > 0
        }
        fn release(&mut self) -> bool {
            for (sig, prev) in self.previous.drain(..) {
                // SAFETY: puts back the handler `signal` returned for this signal.
                unsafe {
                    libc::signal(sig, prev);
                }
            }
            CAUGHT.load(Ordering::SeqCst) > 0
        }
    }
}
