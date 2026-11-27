//! The daemon's watchdog (DMN-621): a runtime whose jobs finish is never called stuck; one whose every job waits on
//! a held lock (the 2026-11-28 freeze's shape) is, once nothing has finished for the limit; a stopped watchdog says
//! nothing of a slow stop; the newest hang reports stay and the next start reads the note once. Short limits stand in
//! for 10 s and 60 s.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferbd::watchdog::{prune, take_note, watch, HUNG_NOTE};
use std::sync::mpsc;

const EVERY: Duration = Duration::from_millis(20);
const HUNG: Duration = Duration::from_millis(300);

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

// decision: DMN-621
#[test]
fn a_runtime_whose_jobs_finish_is_never_called_stuck() {
    let rt = rt();
    let (tx, rx) = mpsc::channel();
    let dog = watch(
        rt.handle().clone(),
        Arc::new(|| {}),
        EVERY,
        HUNG,
        move |d| {
            let _ = tx.send(d);
        },
    );
    assert!(rx.recv_timeout(HUNG * 4).is_err());
    dog.stop();
}

// decision: DMN-621
/// 2026-11-28's shape: every request waits on a lock nobody gives back.
#[test]
fn a_held_database_is_a_hang_once_nothing_has_finished_for_the_limit() {
    let rt = rt();
    let db = Arc::new(Mutex::new(()));
    let probe_db = db.clone();
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    let held = db.lock().unwrap();
    let _dog = watch(
        rt.handle().clone(),
        Arc::new(move || drop(probe_db.lock().unwrap())),
        EVERY,
        HUNG,
        move |d| {
            let _ = tx.send(d);
        },
    );
    let since = rx.recv_timeout(Duration::from_secs(5)).expect("a hang");
    assert!(since >= HUNG && started.elapsed() >= HUNG);
    drop(held);
}

// decision: DMN-621
#[test]
fn a_stopped_watchdog_says_nothing_of_a_slow_stop() {
    let rt = rt();
    let db = Arc::new(Mutex::new(()));
    let probe_db = db.clone();
    let (tx, rx) = mpsc::channel();
    let held = db.lock().unwrap();
    let dog = watch(
        rt.handle().clone(),
        Arc::new(move || drop(probe_db.lock().unwrap())),
        EVERY,
        HUNG,
        move |d| {
            let _ = tx.send(d);
        },
    );
    dog.stop();
    assert!(rx.recv_timeout(HUNG * 3).is_err());
    drop(held);
}

// decision: DMN-621
#[test]
fn the_newest_reports_are_kept_and_the_note_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    for n in 1..=7 {
        std::fs::write(logs.join(format!("hang-2026112{n}-000000.txt")), "x").unwrap();
    }
    std::fs::write(logs.join("daemon.err.log"), "x").unwrap();
    prune(&logs, 5);
    let mut left: Vec<String> = std::fs::read_dir(&logs)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left.len(), 6);
    assert_eq!(left[0], "daemon.err.log");
    assert_eq!(left[1], "hang-20261123-000000.txt");
    let run = dir.path().join("run");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join(HUNG_NOTE),
        r#"{"seconds": 61, "report": "hang-x.txt"}"#,
    )
    .unwrap();
    assert_eq!(take_note(dir.path()).unwrap()["seconds"], 61);
    assert!(take_note(dir.path()).is_none());
}
