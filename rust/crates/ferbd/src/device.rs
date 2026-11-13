//! The internal commands FERB.app calls and the developer toolbox (`ferb <command>`): ferb-device's `cli::run` with
//! this program as its host.
//!
//! Composed here: the version, the daemon connection (the toolbox's daemon clients: `ferb memory`, `ferb ps`,
//! `ferb browsers`...), the salvage check, the clipboard, `ferb logs`' lines, and the setup commands' environment
//! (`setup --service`, `uninstall`, `doctor`: crate::setup_env, the service definition naming `ferbd daemon`).

use std::io::Write;

use ferb_device::cli::client::Client;
use ferb_device::cli::run::{Host, StepLine};
use ferb_device::cli::setup::Env;
use ferb_device::cli::{CliError, Rpc};

struct ProgramHost {
    home: std::path::PathBuf,
    out: std::io::Stdout,
    err: std::io::Stderr,
}

struct Conn(Client);

impl Rpc for Conn {
    fn call(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, CliError> {
        self.0.call(method, params)
    }
    fn close(&mut self) {
        self.0.close()
    }
    fn set_timeout(&mut self, seconds: Option<f64>) {
        self.0.set_timeout(seconds)
    }
    fn stream(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
        on_event: &mut dyn FnMut(serde_json::Value) -> bool,
    ) -> Result<(), CliError> {
        self.0.stream(method, params, on_event)
    }
}

impl Host for ProgramHost {
    fn version(&self) -> String {
        crate::compose::VERSION.into()
    }
    fn out(&mut self) -> &mut dyn Write {
        &mut self.out
    }
    fn err(&mut self) -> &mut dyn Write {
        &mut self.err
    }
    fn own_session(&mut self) {
        ferb_device::cli::own_session(std::env::var("FERB_OWN_SESSION").ok().as_deref());
    }
    fn salvage(&mut self) -> Result<Vec<String>, CliError> {
        let build = ferb_store::salvage::this_build(crate::compose::VERSION);
        let now = ferb_py::time::DateTime::now_local;
        let toml_error = |text: &str| {
            ferb_core::config::Document::parse(text)
                .err()
                .map(|e| e.to_string())
        };
        ferb_store::salvage::check_home(
            &self.home,
            &build,
            &ferb_store::salvage::Seams {
                now: &now,
                toml_error: &toml_error,
            },
        )
        .map(|moved| {
            moved
                .iter()
                .map(|m| {
                    format!(
                        "{} was set aside.",
                        m.get("file").and_then(|v| v.as_str()).unwrap_or("A file")
                    )
                })
                .collect()
        })
        .map_err(|e| CliError::ferb(e.to_string()))
    }
    fn env(&mut self, _every_browser: bool) -> Result<Env<'_>, CliError> {
        Ok(crate::setup_env::env(
            &self.home,
            Box::new(std::io::stdout()),
        ))
    }
    fn connect(&mut self) -> Result<Box<dyn Rpc>, CliError> {
        let socket = ferb_core::paths::socket_path(&self.home);
        Client::connect(&socket, "cli", None, 10.0).map(|c| Box::new(Conn(c)) as Box<dyn Rpc>)
    }
    fn ask(&mut self, prompt: &str) -> String {
        let _ = write!(self.out, "{prompt}");
        let _ = self.out.flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        line.trim_end_matches('\n').to_string()
    }
    fn sleep(&mut self, seconds: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
    }
    fn copy(&mut self, text: &str) -> bool {
        let Ok(mut child) = std::process::Command::new("/usr/bin/pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        else {
            return false;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut i| i.write_all(text.as_bytes()).is_ok());
        wrote && child.wait().is_ok_and(|s| s.success())
    }
    fn step_line(&self) -> Option<StepLine> {
        None
    }
}

/// `ferb <args>`: ferb-device's dispatch with this program as its host. The exit status is Python's.
pub fn run(args: &[String]) -> i32 {
    let home = match crate::home::resolve() {
        Ok((h, _)) => h,
        Err(e) => {
            eprintln!("ferb: {e}");
            return 1;
        }
    };
    let mut host = ProgramHost {
        home,
        out: std::io::stdout(),
        err: std::io::stderr(),
    };
    ferb_device::cli::run::run(args, &mut host)
}
