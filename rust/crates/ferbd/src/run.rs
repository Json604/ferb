//! The program's entry modes, as `ui/mac/FERBAgent/main.c` (four modes) and `ferb/cli` give them, in one program:
//!
//! - `ferbd daemon`: the background service (`python -m ferb.daemon`).
//! - `ferbd mcp`: the models' tool bridge over stdio (`python -I -m ferb.cli.mcp`); the model programs start it
//!   from the runtime factory's `mcp_shim_command`.
//! - anything else: the internal commands FERB.app calls and the developer toolbox (`ferb <command>`,
//!   ferb-device's `cli::run`), e.g. `ferbd setup --service`, `ferbd uninstall --purge --yes`, `ferbd --version`.
//!   Only outside FERB.app: inside it this program is the agent and runs only `daemon`, `mcp` and `grant-probe`
//!   (crate::host); FERB.app's internal commands are the separate `ferb` program ([`cli`]).

use std::sync::Arc;

use crate::compose::{Daemon, Options, StartError};

pub fn main(args: &[String]) -> i32 {
    // inside FERB.app: only the agent's modes, only the owner's home, a rebuilt environment (crate::host, main.c)
    if let Err((status, line)) = crate::host::enter(args) {
        eprintln!("{line}");
        return status;
    }
    match args.first().map(String::as_str) {
        Some("daemon") => {
            crate::log::mark_process_start();
            daemon()
        }
        Some("mcp") => mcp(),
        // `python -m ferb.device.grant_probe` (APP-662): a fresh process's reading of the grants macOS caches per
        // process; the daemon starts this program so for `setup.permissions`
        Some("grant-probe") => {
            let (line, status) = ferb_device::grant_probe::main(&args[1..]);
            println!("{line}");
            status
        }
        _ => crate::device::run(args),
    }
}

/// `ferb <args>`: the program FERB.app ships at `Contents/Resources/bin/ferb` (`src/bin/ferb.rs`), which FERB.app
/// runs for its internal commands (`setup --service`, `uninstall`, `setup --cli`, sign-ins) and the `ferb` link
/// serves as the developer toolbox. It never runs the daemon, the tool bridge or the grant probe: those run as
/// FERB.app's agent ([`main`], crate::host), which holds macOS's permissions; this program holds none.
pub fn cli(args: &[String]) -> i32 {
    crate::device::run(args)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ferbd")
        .build()
        .expect("the daemon's runtime")
}

/// `ferbd mcp` (cli/mcp.py): the run token from the environment, the socket in FERB's home.
fn mcp() -> i32 {
    let home = match crate::home::resolve() {
        Ok((h, _)) => h,
        Err(e) => {
            eprintln!("ferb mcp: {e}");
            return 1;
        }
    };
    runtime().block_on(ferb_runtimes::mcp_bridge::run_stdio(&home))
}

/// `main()` (main.py:739): output to FERB's logs when launchd gave none, the daemon run until SIGTERM, SIGINT or
/// `update.prepare`, then a clean shutdown and exit 0 (launchd's KeepAlive SuccessfulExit false: no restart).
fn daemon() -> i32 {
    let home = match crate::home::resolve() {
        Ok((h, _)) => h,
        Err(e) => {
            eprintln!("FERB: {e}");
            return 1;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&home) {
        eprintln!("FERB: can't make FERB's home: {e}");
        return 1;
    }
    if let Err(e) = crate::home::service_streams(&home) {
        eprintln!("FERB: can't open FERB's logs: {e}");
    }
    crate::log::install_panic_hook();
    crate::log::line(&format!(
        "starting: pid {}, FERB {}, build {}, started by {}",
        std::process::id(),
        crate::compose::VERSION,
        std::env::var("FERB_APP_BUILD").unwrap_or_else(|_| "none".into()),
        std::env::var("XPC_SERVICE_NAME")
            .ok()
            .filter(|s| !s.is_empty() && s != "0")
            .map(|s| format!("launchd ({s})"))
            .unwrap_or_else(|| "a command".into())
    ));
    #[allow(unused_mut)]
    let mut opts = Options::real(home);
    #[cfg(feature = "stand-ins")]
    let stand = crate::standins::from_env(&mut opts);
    // the async work on a thread of its own; the main thread turns macOS's run loop (`_macos_events`)
    crate::mainloop::run("ferbd-daemon", move || {
        runtime().block_on(async move {
            use tokio::signal::unix::{signal, SignalKind};
            // the signal handlers come first: a stop asked while FERB is still starting shuts it down once started
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
            let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
            // a hang-up (the shell that started a development daemon closing) stops it like SIGTERM: never left behind
            let mut hup = signal(SignalKind::hangup()).expect("SIGHUP handler");
            let stop = Arc::new(tokio::sync::Notify::new());
            {
                let stop = stop.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = term.recv() => {}
                        _ = int.recv() => {}
                        _ = hup.recv() => {}
                    }
                    stop.notify_one();
                });
            }
            let daemon = match Daemon::start(opts).await {
                Ok(d) => d,
                Err(StartError::NewerStore(m)) => {
                    // not a crash: nothing was changed, and a clean exit keeps launchd from starting it again
                    crate::log::line(&m);
                    return 0;
                }
                Err(e) => {
                    crate::log::line(&format!("the daemon didn't start: {e}"));
                    return 1;
                }
            };
            #[cfg(feature = "stand-ins")]
            if let Some(s) = stand {
                crate::standins::attach(&daemon, s).await;
            }
            let quit = daemon.quit.clone();
            tokio::select! {
                _ = stop.notified() => {}
                _ = quit.notified() => {}
            }
            crate::log::line("stopping (asked to)");
            daemon.shutdown().await;
            crate::log::line("stopped");
            0
        })
    })
}
