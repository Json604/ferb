//! `_macos_events` (main.py:642): macOS's event system gets a turn on the main thread a few times a second. Without
//! it the running-app list never refreshes in the daemon (an app launched after the daemon started stays "not
//! running" for FERB: the owner's WhatsApp report, 2026-10-01, and this port's real-machine check, where TextEdit
//! opened by mac.open was then "not running"). NSWorkspace's `runningApplications` and each NSRunningApplication's
//! state change only when the main run loop runs in a common mode (Apple's documentation of the property); measured
//! on this Mac 2026-11-13 (rust/JOURNAL.md 46): with no turn, a launched TextEdit stayed off the list for all of 6 s
//! of looking; with these turns it was listed within 0.25 s. Python's asyncio loop runs on the main thread and turns
//! the run loop there; here the daemon's async work runs on a thread of its own and the main thread turns the run
//! loop.

/// MACOS_TICK_S (areas/shared.py:18).
pub const MACOS_TICK_S: f64 = 0.2;

/// Runs `work` on a thread named `name` while the main thread gives macOS's run loop a zero-wait turn every
/// MACOS_TICK_S (each turn in its own autorelease pool, DMN-26); returns what `work` returns.
pub fn run<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    run_with(name, MACOS_TICK_S, turn, work)
}

/// One zero-wait turn of the calling thread's run loop (the main thread's, from `run`), in its own pool: the
/// sources and the main dispatch queue get their callbacks, NSWorkspace's list its updates.
pub fn turn() {
    #[cfg(target_os = "macos")]
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: the exported mode name; the run loop is the calling thread's own (`run` calls it on the main thread).
        let mode = unsafe { objc2_core_foundation::kCFRunLoopDefaultMode };
        objc2_core_foundation::CFRunLoop::run_in_mode(mode, 0.0, true);
    });
}

/// `run` with its tick and its turn given: `turn` runs on the calling thread, before each wait of `tick_s`, until
/// `work` (on its own thread) is done.
pub fn run_with<T: Send + 'static>(
    name: &str,
    tick_s: f64,
    turn: impl Fn(),
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let job = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
        .expect("the daemon's thread");
    while !job.is_finished() {
        turn();
        std::thread::sleep(std::time::Duration::from_secs_f64(tick_s));
    }
    match job.join() {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex};

    /// The stale-list guard: the run loop is turned on the thread that called `run` (the daemon's main thread) all the
    /// while the daemon's work runs elsewhere, so macOS's updates reach NSWorkspace's list during that work.
    #[test]
    fn the_calling_thread_turns_the_run_loop_while_the_work_runs() {
        let caller = std::thread::current().id();
        let turns: Arc<(Mutex<Vec<std::thread::ThreadId>>, Condvar)> = Arc::default();
        let seen = turns.clone();
        let out = super::run_with(
            "ferbd-test-work",
            0.0,
            move || {
                let (m, cv) = &*seen;
                m.lock().unwrap().push(std::thread::current().id());
                cv.notify_all();
            },
            {
                let turns = turns.clone();
                move || {
                    // the work waits for three turns made while it runs: never for a fixed time
                    let (m, cv) = &*turns;
                    let g = cv.wait_while(m.lock().unwrap(), |t| t.len() < 3).unwrap();
                    drop(g);
                    (
                        std::thread::current().name().map(String::from),
                        std::thread::current().id(),
                    )
                }
            },
        );
        assert_eq!(out.0.as_deref(), Some("ferbd-test-work"));
        assert_ne!(out.1, caller, "the work runs on a thread of its own");
        let made = turns.0.lock().unwrap();
        assert!(made.len() >= 3);
        assert!(
            made.iter().all(|t| *t == caller),
            "every turn on the calling thread"
        );
    }
}
