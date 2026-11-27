//! The daemon's watchdog (owner, 2026-11-28). That night the daemon stayed alive but
//! answered nothing for hours: two of its locks waited on each other, every request queued behind them, and launchd,
//! which restarts only a daemon that exits, never knew. The watchdog is a thread of its own, outside the async
//! runtime: every [`PROBE_EVERY`] it hands the runtime a small job that takes what every request takes (the
//! database). When no job has finished for [`HUNG_AFTER`], the daemon is stuck: it writes what each of its threads
//! is doing (`/usr/bin/sample`) to `logs/hang-<time>.txt`, leaves `run/hung.json` for the next start to journal
//! (`daemon.hung`: nothing can be journaled while the database is held), and exits [`EXIT_HUNG`], which launchd
//! answers by starting it again (KeepAlive SuccessfulExit false). Work that was running then is cut at the next
//! start as after any restart. Its clock is `Instant`, which stops while the Mac sleeps, so a sleep is never a hang.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// How often the runtime is handed a job.
pub const PROBE_EVERY: Duration = Duration::from_secs(10);
/// How long without a finished job is a hang (the owner's "60 seconds", 2026-11-28).
pub const HUNG_AFTER: Duration = Duration::from_secs(60);
/// The hang reports kept in `logs/` (each about 100 KB); the oldest goes when a newer one is written.
pub const KEEP_REPORTS: usize = 5;
/// The exit code of a stuck daemon: not 0, so launchd starts it again.
pub const EXIT_HUNG: i32 = 70;
/// How long `sample` may take before the daemon exits without its report.
const SAMPLE_LIMIT: Duration = Duration::from_secs(30);

/// A running watchdog. [`Watchdog::stop`] before a stop on purpose: a slow shutdown is not a hang.
pub struct Watchdog {
    stopped: Arc<AtomicBool>,
}

impl Watchdog {
    pub fn stop(&self) {
        self.stopped.store(true, SeqCst);
    }
}

/// Watches `runtime`: `probe` runs there every `every`; when none has finished for `hung_after`, `on_hang` runs on the
/// watchdog's thread with how long it has been, and the watchdog ends.
pub fn watch(
    runtime: tokio::runtime::Handle,
    probe: Arc<dyn Fn() + Send + Sync>,
    every: Duration,
    hung_after: Duration,
    on_hang: impl FnOnce(Duration) + Send + 'static,
) -> Watchdog {
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let base = Instant::now();
    // the last finished job, as milliseconds after `base`
    let beat = Arc::new(AtomicU64::new(0));
    let in_flight = Arc::new(AtomicBool::new(false));
    let spawned = std::thread::Builder::new()
        .name("ferbd-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(every);
            if stop.load(SeqCst) {
                return;
            }
            // one job at a time: a stuck one is not joined by more
            if !in_flight.swap(true, SeqCst) {
                let (probe, beat, in_flight) = (probe.clone(), beat.clone(), in_flight.clone());
                runtime.spawn(async move {
                    probe();
                    beat.store(base.elapsed().as_millis() as u64, SeqCst);
                    in_flight.store(false, SeqCst);
                });
            }
            let since = base
                .elapsed()
                .saturating_sub(Duration::from_millis(beat.load(SeqCst)));
            if since >= hung_after && !stop.load(SeqCst) {
                on_hang(since);
                return;
            }
        });
    if let Err(e) = spawned {
        crate::log::line(&format!("the watchdog didn't start: {e}"));
    }
    Watchdog { stopped }
}

/// The real daemon's answer to a hang: the report, the note for the next start, the exit.
pub fn hung(home: &Path, since: Duration) -> ! {
    let stamp = chrono::Local::now();
    let name = format!("hang-{}.txt", stamp.format("%Y%m%d-%H%M%S"));
    let logs = home.join("logs");
    crate::log::line(&format!(
        "not answering for {} s: writing what each thread does to logs/{name}, then starting again",
        since.as_secs()
    ));
    let written = sample(&logs.join(&name));
    prune(&logs, KEEP_REPORTS);
    let note = json!({
        "at": stamp.to_rfc3339(),
        "seconds": since.as_secs(),
        "report": if written { Value::String(name) } else { Value::Null },
    });
    let _ = std::fs::write(home.join("run").join(HUNG_NOTE), note.to_string());
    crate::log::line("stuck: exiting for launchd to start FERB again");
    std::process::exit(EXIT_HUNG)
}

/// Where a hang is noted for the next start.
pub const HUNG_NOTE: &str = "hung.json";

/// The note a hang left (removed as it is read): `daemon.hung`'s data.
pub fn take_note(home: &Path) -> Option<Map<String, Value>> {
    let path = home.join("run").join(HUNG_NOTE);
    let text = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    match serde_json::from_str(&text?) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// `sample <pid> 3` into `path`, within [`SAMPLE_LIMIT`]; whether it was written.
fn sample(path: &Path) -> bool {
    let pid = std::process::id().to_string();
    let Ok(mut child) = Command::new("/usr/bin/sample")
        .args([pid.as_str(), "3", "-mayDie", "-file"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let until = Instant::now() + SAMPLE_LIMIT;
    while Instant::now() < until {
        match child.try_wait() {
            Ok(Some(status)) => return status.success() && path.exists(),
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

/// Keeps the newest `keep` hang reports in `logs` (their names sort by time).
pub fn prune(logs: &Path, keep: usize) {
    let mut reports: Vec<PathBuf> = std::fs::read_dir(logs)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("hang-") && n.ends_with(".txt"))
        })
        .collect();
    reports.sort();
    let extra = reports.len().saturating_sub(keep);
    for old in &reports[..extra] {
        let _ = std::fs::remove_file(old);
    }
}
