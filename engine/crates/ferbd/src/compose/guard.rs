//! The daemon's background work, supervised (main.py:596-631 `_spawn` and `_supervise`, T-1123).
//!
//! - A loop meant to run for the daemon's life ([`Guard::run`], [`Guard::run_blocking`] for one on a thread of its
//!   own): when its body returns an error or panics, the failure is journaled `daemon.loop_failed` (the loop's name,
//!   the failures so far, the error's kind and its redacted words, as Python's) and the loop starts again after a
//!   pause that doubles from LOOP_RESTART_S up to LOOP_RESTART_MAX_S, so a loop that keeps failing starts at most once
//!   a minute. A body that returns Ok ended by itself (Python's `return`): it isn't started again. The body is told
//!   how many times it failed before, so a periodic loop waits its period before the pass after a failure instead of
//!   running it again at once (a pass that calls a model never repeats every minute).
//! - One-off work ([`Guard::background`]): a failure or panic is journaled `daemon.background_failed` (what, error).
//!
//! Python's counter is never reset by a run that went well (its docstring says "in a row"; its code counts every
//! failure since the start): kept as the code has it. A panic was Python's uncaught exception: its words are the
//! panic's message. The panic hook journals it as `daemon.unhandled` too (src/log.rs). Shutdown aborts a loop's task,
//! in its body or during its pause alike; a loop on a thread stops at its own stop check, also during a pause.
//!
//! Every loop and one-off job of the composition goes through here: `tests/loops_supervised.rs` reads src/compose and
//! fails on a spawn that isn't this module's or listed in `spawns.txt` as per-call work.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use futures_util::FutureExt;
use serde_json::{json, Value};

/// `LOOP_RESTART_S`, `LOOP_RESTART_MAX_S` (main.py:93).
pub const LOOP_RESTART_S: f64 = 1.0;
pub const LOOP_RESTART_MAX_S: f64 = 60.0;
/// The journal's kinds (main.py:609, 625).
pub const LOOP_FAILED: &str = "daemon.loop_failed";
pub const BACKGROUND_FAILED: &str = "daemon.background_failed";

/// The pause before a loop starts again after its `failures`-th failure (main.py:631).
pub fn restart_pause(failures: u32) -> f64 {
    let doubled = LOOP_RESTART_S * 2f64.powi(failures.saturating_sub(1).min(64) as i32);
    doubled.min(LOOP_RESTART_MAX_S)
}

/// How a supervised loop waits (time injected: the real daemon's sleeps, a test's recorder).
pub trait Pause: Send + Sync {
    fn sleep(&self, seconds: f64) -> BoxFuture<'static, ()>;
    /// On a thread of its own: returns early once `stop` says so (checked at least once a second).
    fn sleep_blocking(&self, seconds: f64, stop: &dyn Fn() -> bool);
}

/// The real daemon's pauses.
pub struct RealPause;

impl Pause for RealPause {
    fn sleep(&self, seconds: f64) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(
            seconds.max(0.0),
        )))
    }
    fn sleep_blocking(&self, seconds: f64, stop: &dyn Fn() -> bool) {
        let until =
            std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds.max(0.0));
        while !stop() {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(std::time::Duration::from_secs(1)));
        }
    }
}

/// Where a failure is journaled: `journal.note(kind, data)`.
pub type Note = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Where a failure is journaled, and how its words are redacted.
#[derive(Clone)]
pub struct Guard {
    note: Note,
    redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    pub pause: Arc<dyn Pause>,
}

/// A panic's message (its payload when it is text).
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(a panic without a message)".into())
}

impl Guard {
    pub fn new(
        note: Note,
        redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
        pause: Arc<dyn Pause>,
    ) -> Guard {
        Guard {
            note,
            redact,
            pause,
        }
    }

    /// The daemon's journal and redactor.
    pub fn of(
        journal: &Arc<crate::basics::Journal>,
        redactor: &Arc<ferb_sanitize::redact::Redactor>,
        pause: &Arc<dyn Pause>,
    ) -> Guard {
        let (j, r) = (journal.clone(), redactor.clone());
        Guard::new(
            Arc::new(move |kind: &str, data: Value| j.note(kind, data)),
            Arc::new(move |t: &str| r.text(t)),
            pause.clone(),
        )
    }

    /// `self._redact_text(f"{type(e).__name__}: {e}")[:INTERNAL_MESSAGE_CHARS]`.
    fn words(&self, error: &str) -> String {
        (self.redact)(error)
            .chars()
            .take(ferb_daemon::shared::INTERNAL_MESSAGE_CHARS)
            .collect()
    }

    fn loop_failed(&self, name: &str, failures: u32, error: &str) {
        let data = json!({"loop": name, "failures": failures, "error": self.words(error)});
        let note = self.note.clone();
        // `contextlib.suppress(Exception)`: a journal that fails never stops the loop
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| note(LOOP_FAILED, data)));
    }

    /// `_supervise(name, loop)`: `body(failures so far)` until it returns Ok; each error or panic journaled and the
    /// body started again after [`restart_pause`].
    pub async fn run<F, Fut>(&self, name: &str, mut body: F)
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        let mut failures = 0u32;
        loop {
            let error = match std::panic::catch_unwind(AssertUnwindSafe(|| body(failures))) {
                Ok(fut) => match AssertUnwindSafe(fut).catch_unwind().await {
                    Ok(Ok(())) => return,
                    Ok(Err(e)) => e,
                    Err(p) => format!("panic: {}", panic_text(p.as_ref())),
                },
                Err(p) => format!("panic: {}", panic_text(p.as_ref())),
            };
            failures = failures.saturating_add(1);
            self.loop_failed(name, failures, &error);
            self.pause.sleep(restart_pause(failures)).await;
        }
    }

    /// [`Guard::run`] for a loop on a thread of its own; `stop` ends it (before a start, during a pause).
    pub fn run_blocking(
        &self,
        name: &str,
        stop: &dyn Fn() -> bool,
        mut body: impl FnMut(u32) -> Result<(), String>,
    ) {
        let mut failures = 0u32;
        while !stop() {
            let error = match std::panic::catch_unwind(AssertUnwindSafe(|| body(failures))) {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e,
                Err(p) => format!("panic: {}", panic_text(p.as_ref())),
            };
            failures = failures.saturating_add(1);
            self.loop_failed(name, failures, &error);
            self.pause.sleep_blocking(restart_pause(failures), stop);
        }
    }

    /// `_spawn(work, what)`'s failure: journaled `daemon.background_failed`, never lost.
    pub async fn background<Fut>(&self, what: &str, work: Fut)
    where
        Fut: Future<Output = Result<(), String>>,
    {
        match AssertUnwindSafe(work).catch_unwind().await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => self.background_failed(what, &e),
            Err(p) => self.background_failed(what, &format!("panic: {}", panic_text(p.as_ref()))),
        }
    }

    /// [`Guard::background`] for one-off work on a thread of its own.
    pub fn background_blocking(&self, what: &str, work: impl FnOnce() -> Result<(), String>) {
        match std::panic::catch_unwind(AssertUnwindSafe(work)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => self.background_failed(what, &e),
            Err(p) => self.background_failed(what, &format!("panic: {}", panic_text(p.as_ref()))),
        }
    }

    fn background_failed(&self, what: &str, error: &str) {
        let data = json!({"what": what, "error": self.words(error)});
        let note = self.note.clone();
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| note(BACKGROUND_FAILED, data)));
    }
}

/// A loop on a thread of its own named `thread`, supervised ([`Guard::run_blocking`]); `stop` ends it.
pub fn thread(
    guard: Guard,
    thread: &str,
    name: &'static str,
    stop: impl Fn() -> bool + Send + 'static,
    body: impl FnMut(u32) -> Result<(), String> + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(thread.into())
        .spawn(move || guard.run_blocking(name, &stop, body))
}

/// One-off work on a thread of its own named `thread` ([`Guard::background_blocking`]).
pub fn thread_once(
    guard: Guard,
    thread: &str,
    what: &'static str,
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(thread.into())
        .spawn(move || guard.background_blocking(what, work))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    type Rows = Arc<Mutex<Vec<(String, Value)>>>;

    /// A pause that records what it was asked and returns at once; `hold` makes the async one never end.
    #[derive(Default)]
    struct Recorded {
        asked: Mutex<Vec<f64>>,
        hold: bool,
    }

    impl Pause for Recorded {
        fn sleep(&self, seconds: f64) -> BoxFuture<'static, ()> {
            self.asked.lock().unwrap().push(seconds);
            if self.hold {
                Box::pin(std::future::pending())
            } else {
                Box::pin(async {})
            }
        }
        fn sleep_blocking(&self, seconds: f64, _stop: &dyn Fn() -> bool) {
            self.asked.lock().unwrap().push(seconds);
        }
    }

    fn guard(pause: Arc<Recorded>) -> (Guard, Rows) {
        let rows: Rows = Arc::default();
        let r = rows.clone();
        let g = Guard::new(
            Arc::new(move |kind: &str, data: Value| r.lock().unwrap().push((kind.into(), data))),
            Arc::new(|t: &str| t.replace("secret", "[redacted]")),
            pause,
        );
        (g, rows)
    }

    // decision: DMN-599
    #[test]
    fn the_pause_doubles_from_one_second_up_to_a_minute() {
        let pauses: Vec<f64> = (1..=9).map(restart_pause).collect();
        assert_eq!(pauses, [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0, 60.0]);
        assert_eq!(restart_pause(u32::MAX), 60.0);
    }

    // decision: DMN-599
    #[tokio::test]
    async fn an_error_is_journaled_and_the_loop_starts_again() {
        let pause = Arc::new(Recorded::default());
        let (g, rows) = guard(pause.clone());
        let calls = Arc::new(AtomicU32::new(0));
        let seen = calls.clone();
        g.run("timers", move |failures| {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(
                    failures, n,
                    "the body is told how many times it failed before"
                );
                if n < 3 {
                    Err(format!(
                        "OperationalError: the secret store is locked ({n})"
                    ))
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "started again after each failure, ended by itself"
        );
        let rows = rows.lock().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, LOOP_FAILED);
        assert_eq!(
            rows[0].1,
            json!({"loop": "timers", "failures": 1,
                   "error": "OperationalError: the [redacted] store is locked (0)"})
        );
        assert_eq!(rows[2].1["failures"], 3);
        assert_eq!(*pause.asked.lock().unwrap(), [1.0, 2.0, 4.0]);
    }

    // decision: DMN-599
    #[tokio::test]
    async fn a_panic_is_journaled_and_the_loop_starts_again() {
        let pause = Arc::new(Recorded::default());
        let (g, rows) = guard(pause.clone());
        let calls = Arc::new(AtomicU32::new(0));
        let seen = calls.clone();
        g.run("held_banners", move |_| {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    panic!("the held banners' lock was poisoned");
                }
                Ok(())
            }
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let first = rows.lock().unwrap()[0].1.clone();
        assert_eq!(
            first,
            json!({"loop": "held_banners", "failures": 1,
                   "error": "panic: the held banners' lock was poisoned"})
        );
        // a panic while the body is made (before its future runs) counts the same
        let made = Arc::new(AtomicU32::new(0));
        let m = made.clone();
        g.run("made", move |_| {
            if m.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("no parts");
            }
            async { Ok(()) }
        })
        .await;
        assert_eq!(made.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_loop_that_keeps_failing_waits_longer_each_time_never_more_than_a_minute() {
        let pause = Arc::new(Recorded::default());
        let (g, rows) = guard(pause.clone());
        let calls = Arc::new(AtomicU32::new(0));
        let seen = calls.clone();
        g.run("embedder", move |_| {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 10 {
                    Err("RuntimeError: again".to_string())
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert_eq!(
            *pause.asked.lock().unwrap(),
            [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0, 60.0, 60.0]
        );
        assert_eq!(rows.lock().unwrap()[9].1["failures"], 10);
    }

    /// Shutdown aborts the loop's task while it waits to start again: the body never runs again and the task ends.
    // decision: DMN-599
    #[tokio::test]
    async fn a_shutdown_during_the_pause_stops_the_loop() {
        let pause = Arc::new(Recorded {
            hold: true,
            ..Recorded::default()
        });
        let (g, rows) = guard(pause.clone());
        let calls = Arc::new(AtomicU32::new(0));
        let seen = calls.clone();
        let job = tokio::spawn(async move {
            g.run("retention", move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                async { Err("OSError: gone".to_string()) }
            })
            .await
        });
        while pause.asked.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(rows.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_loop_on_its_own_thread_is_supervised_and_stops_when_asked() {
        let pause = Arc::new(Recorded::default());
        let (g, rows) = guard(pause.clone());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s1, s2) = (stop.clone(), stop.clone());
        let calls = Arc::new(AtomicU32::new(0));
        let seen = calls.clone();
        thread(
            g,
            "ferb-test-loop",
            "trail",
            move || s1.load(Ordering::SeqCst),
            move |n| {
                seen.fetch_add(1, Ordering::SeqCst);
                match n {
                    0 => panic!("trail"),
                    1 => Err("OperationalError: locked".into()),
                    // the stop comes during the pause after this failure: the body isn't run again
                    _ => {
                        s2.store(true, Ordering::SeqCst);
                        Err("OperationalError: again".into())
                    }
                }
            },
        )
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let rows = rows.lock().unwrap();
        assert_eq!(rows[0].1["error"], "panic: trail");
        assert_eq!(rows[1].1["error"], "OperationalError: locked");
        assert_eq!(rows.len(), 3);
        assert_eq!(*pause.asked.lock().unwrap(), [1.0, 2.0, 4.0]);
    }

    #[tokio::test]
    async fn one_off_work_that_fails_is_journaled_as_background_failed() {
        let (g, rows) = guard(Arc::new(Recorded::default()));
        g.background("browser files", async {
            Err("OSError: no access".to_string())
        })
        .await;
        g.background("permission check", async {
            panic!("tcc");
        })
        .await;
        g.background("fine", async { Ok(()) }).await;
        g.background_blocking("search re-index", || Err("OperationalError: busy".into()));
        let rows = rows.lock().unwrap();
        assert_eq!(
            *rows,
            [
                (
                    BACKGROUND_FAILED.to_string(),
                    json!({"what": "browser files", "error": "OSError: no access"})
                ),
                (
                    BACKGROUND_FAILED.to_string(),
                    json!({"what": "permission check", "error": "panic: tcc"})
                ),
                (
                    BACKGROUND_FAILED.to_string(),
                    json!({"what": "search re-index", "error": "OperationalError: busy"})
                ),
            ]
        );
    }

    /// A journal that fails never stops the loop (`contextlib.suppress`).
    #[tokio::test]
    async fn a_journal_that_fails_never_stops_the_loop() {
        let g = Guard::new(
            Arc::new(|_: &str, _: Value| panic!("the store is gone")),
            Arc::new(str::to_string),
            Arc::new(Recorded::default()),
        );
        let calls = Arc::new(AtomicU32::new(0));
        let seen = calls.clone();
        g.run("x", move |n| {
            seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err("E: x".to_string())
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
