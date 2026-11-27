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
    // HAND-531: the fresh process that reads an image's text when the daemon's own Vision can't
    if args.len() == 1 && args[0] == ferb_mac_hands::vision_child::COMMAND {
        return ferb_mac_hands::native::vision::child_main();
    }
    // HAND-626: the kept child, framed images until its input closes
    if args.len() == 2
        && args[0] == ferb_mac_hands::vision_child::COMMAND
        && args[1] == ferb_mac_hands::vision_child::SERVE
    {
        return ferb_mac_hands::native::vision::serve_main();
    }
    // HAND-718: the archived sessions' Claude Code hook (`bin/ferb agent-hook claude <Event>`): a stub, exit 0, no output
    if args.first().map(String::as_str) == Some(crate::agent_hook::COMMAND) {
        return crate::agent_hook::main(args);
    }
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

/// `main()` (main.py:739): output to FERB's logs when launchd gave none, the daemon run until SIGTERM, SIGINT, SIGHUP or
/// `update.prepare`, then a clean shutdown. The exit code tells launchd (KeepAlive SuccessfulExit false) whether to
/// start it again (DMN-618, DMN-619): 0 only for FERB's own stops (`update.prepare`; a signal while the stop marker
/// names this process, ferb-device `Installer::stop`, or while the owner's Settings turned FERB off, APP-1110), which
/// stay stopped until FERB starts it; any other stop (a
/// `kill`, Activity Monitor, macOS's Quit & Reopen) exits 128 + the signal, and launchd brings it back within its
/// 5 s throttle. Python exited 0 on every signal.
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
    // the owner turned FERB off in Settings (APP-1110): nothing starts until FERB.app opens and removes the file
    if ferb_device::install::turned_off(&home) {
        crate::log::line("turned off in FERB's Settings: starts again when FERB.app opens");
        return 0;
    }
    let stop_home = home.clone();
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
            // the signal that stopped it (0: none, `update.prepare`)
            let signalled = Arc::new(std::sync::atomic::AtomicI32::new(0));
            {
                let (stop, signalled) = (stop.clone(), signalled.clone());
                tokio::spawn(async move {
                    let signo = tokio::select! {
                        _ = term.recv() => libc::SIGTERM,
                        _ = int.recv() => libc::SIGINT,
                        _ = hup.recv() => libc::SIGHUP,
                    };
                    signalled.store(signo, std::sync::atomic::Ordering::SeqCst);
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
            // this daemon holds FERB's home now: a stop marker an earlier daemon's stop left never makes it stay stopped
            ferb_device::install::clear_stale_stop(&stop_home, i64::from(std::process::id()));
            // the last daemon was stuck and the watchdog ended it: its note is journaled now
            if let Some(note) = crate::watchdog::take_note(&stop_home) {
                crate::log::line("the last run was stuck and was started again (daemon.hung)");
                daemon
                    .journal
                    .note("daemon.hung", serde_json::Value::Object(note));
            }
            let watchdog = {
                let (db, home) = (daemon.db.clone(), stop_home.clone());
                crate::watchdog::watch(
                    tokio::runtime::Handle::current(),
                    Arc::new(move || drop(crate::basics::lock(&db))),
                    crate::watchdog::PROBE_EVERY,
                    crate::watchdog::HUNG_AFTER,
                    move |since| crate::watchdog::hung(&home, since),
                )
            };
            #[cfg(feature = "stand-ins")]
            if let Some(s) = stand {
                crate::standins::attach(&daemon, s).await;
            }
            let quit = daemon.quit.clone();
            // `update.prepare`: FERB's own stop for its update
            let updating = tokio::select! {
                _ = stop.notified() => false,
                _ = quit.notified() => true,
            };
            crate::log::line("stopping (asked to)");
            watchdog.stop(); // a slow stop is not a hang
            daemon.shutdown().await;
            let signo = signalled.load(std::sync::atomic::Ordering::SeqCst);
            let on_purpose = updating
                || signo == 0
                || ferb_device::install::turned_off(&stop_home)
                || ferb_device::install::stopped_on_purpose(
                    &stop_home,
                    i64::from(std::process::id()),
                );
            if on_purpose {
                crate::log::line("stopped");
                0
            } else {
                crate::log::line("stopped (not by FERB: launchd starts it again)");
                128 + signo
            }
        })
    })
}
