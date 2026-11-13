//! The program's entry modes, as `ui/mac/FERBAgent/main.c` (four modes) and `ferb/cli` give them, in one program:
//!
//! - `ferbd daemon`: the background service (`python -m ferb.daemon`).
//! - `ferbd mcp`: the models' tool bridge over stdio (`python -I -m ferb.cli.mcp`); the model programs start it
//!   from the runtime factory's `mcp_shim_command`.
//! - anything else: the internal commands FERB.app calls and the developer toolbox (`ferb <command>`,
//!   ferb-device's `cli::run`), e.g. `ferbd setup --service`, `ferbd uninstall --purge --yes`, `ferbd --version`.

use std::sync::Arc;

use crate::compose::{Daemon, Options, StartError};

pub fn main(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("daemon") => daemon(),
        Some("mcp") => mcp(),
        _ => crate::device::run(args),
    }
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
    #[allow(unused_mut)]
    let mut opts = Options::real(home);
    #[cfg(feature = "stand-ins")]
    let stand = crate::standins::from_env(&mut opts);
    let rt = runtime();
    rt.block_on(async move {
        use tokio::signal::unix::{signal, SignalKind};
        // the signal handlers come first: a stop asked while FERB is still starting shuts it down once started
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        let stop = Arc::new(tokio::sync::Notify::new());
        {
            let stop = stop.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = int.recv() => {}
                }
                stop.notify_one();
            });
        }
        let daemon = match Daemon::start(opts).await {
            Ok(d) => d,
            Err(StartError::NewerStore(m)) => {
                // not a crash: nothing was changed, and a clean exit keeps launchd from starting it again
                eprintln!("FERB: {m}");
                return 0;
            }
            Err(e) => {
                eprintln!("FERB: the daemon didn't start: {e}");
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
        daemon.shutdown().await;
        0
    })
}
